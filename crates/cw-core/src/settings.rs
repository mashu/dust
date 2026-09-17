//! Training settings for the group trainer.
//!
//! The `Default` impls are written out rather than derived: every default is a
//! decision about how the trainer behaves out of the box, and they read better
//! spelled out next to the type than as an attribute on one variant.
#![allow(clippy::derivable_impls)]

use serde::{Deserialize, Serialize};

use crate::level::{max_level_for_len, LEVEL_MIN};
use crate::morse::{is_digit, morse_for, DEFAULT_SLIDING_WINDOW_END, DEFAULT_SLIDING_WINDOW_START};

/// A group is always sent at least once.
pub const GROUP_REPEAT_MIN: u32 = 1;
/// Upper bound for "send the group N times before the answer window opens".
pub const GROUP_REPEAT_MAX: u32 = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MixedAutoLevelAxis {
    Letters,
    Digits,
}

impl Default for MixedAutoLevelAxis {
    fn default() -> Self {
        Self::Letters
    }
}

impl MixedAutoLevelAxis {
    pub fn flip(self) -> Self {
        match self {
            Self::Letters => Self::Digits,
            Self::Digits => Self::Letters,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReceiverProfile {
    Whistle,
    Ringing,
    Mixed,
}

impl Default for ReceiverProfile {
    fn default() -> Self {
        Self::Mixed
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CharSetMode {
    Koch,
    Digits,
    Custom,
    Mixed,
    /// Realistic amateur callsigns instead of drawn groups. The characters are
    /// not what progresses here — the shape of the call is.
    Callsign,
}

impl Default for CharSetMode {
    fn default() -> Self {
        Self::Mixed
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PracticeWindow {
    All,
    Last3,
    Last5,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CurriculumSettings {
    /// Progress through the current letter/custom alphabet (level 1 unlocks two characters).
    #[serde(alias = "kochLevel", default = "defaults::level")]
    pub level: u32,
    pub char_set_mode: CharSetMode,
    pub digits_level: u32,
    /// When mixed: 0–100 percent of characters that are letters (rest digits).
    pub mixed_letters_percent: u32,
    pub custom_set: Vec<char>,
    pub custom_sequence: Vec<char>,
    /// True when the user chose Sequence → Custom, even if the order still matches a preset.
    #[serde(default)]
    pub sequence_is_custom: bool,
    /// Named practice window. `None` means infer from the saved start/end (old saves).
    #[serde(default)]
    pub practice_window: Option<PracticeWindow>,
    pub sliding_window_start: u32,
    pub sliding_window_end: u32,
    /// Callsign structure tier, the level of the callsign character set. Its own
    /// field, the way digits have one, so switching modes never reinterprets a
    /// number that meant something else.
    #[serde(default = "defaults::level")]
    pub callsign_level: u32,
    pub num_groups: u32,
    pub min_group_size: u32,
    pub max_group_size: u32,
    pub link_group_size: bool,
}

impl Default for CurriculumSettings {
    fn default() -> Self {
        Self {
            level: LEVEL_MIN,
            char_set_mode: CharSetMode::Mixed,
            digits_level: 1,
            mixed_letters_percent: 70,
            custom_set: Vec::new(),
            custom_sequence: Vec::new(),
            sequence_is_custom: false,
            practice_window: Some(PracticeWindow::All),
            callsign_level: LEVEL_MIN,
            sliding_window_start: DEFAULT_SLIDING_WINDOW_START,
            sliding_window_end: DEFAULT_SLIDING_WINDOW_END,
            num_groups: 20,
            min_group_size: 3,
            max_group_size: 5,
            link_group_size: false,
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
    pub extra_word_space_multiplier: f64,
    pub group_timeout: f64,
    pub lock_input_during_group_playback: bool,
    /// Lowest number of times a group is sent before the answer window opens.
    #[serde(default = "defaults::group_repeat")]
    pub group_repeat_min: u32,
    #[serde(default = "defaults::group_repeat")]
    pub group_repeat_max: u32,
    #[serde(default = "defaults::enabled")]
    pub link_group_repeat: bool,
}

impl Default for PlaybackSettings {
    fn default() -> Self {
        Self {
            char_wpm_min: 18.0,
            char_wpm_max: 25.0,
            link_char_wpm: false,
            effective_wpm_min: 18.0,
            effective_wpm_max: 25.0,
            link_effective_wpm: false,
            link_char_to_effective: true,
            extra_word_space_multiplier: 1.0,
            group_timeout: 10.0,
            lock_input_during_group_playback: true,
            group_repeat_min: 1,
            group_repeat_max: 1,
            link_group_repeat: true,
        }
    }
}

/// The narrowest and widest the receiver goes, in hertz. The bottom is the
/// classic narrow CW position; the top is wide open, where the filter stops
/// being the thing you notice.
pub const FILTER_BANDWIDTH_MIN: f64 = 150.0;
pub const FILTER_BANDWIDTH_MAX: f64 = 2_000.0;

/// How many stations can be calling at once. One is just the station you want;
/// above that the others are QRM, and the count is drawn fresh for each group
/// so a pile-up never arrives the same way twice.
/// How far the receiver-character model's gain can be pushed.
///
/// This used to be 20, which was also its default — so the slider could only
/// ever come down from where it started, and anyone wanting more character
/// found it already against the stop. The model feeds the same soft limiter as
/// everything else, so the top of the range is loud rather than broken.
pub const RECEIVER_MODEL_GAIN_MAX: f64 = 80.0;

pub const STATIONS_MIN: u32 = 1;
pub const STATIONS_MAX: u32 = 5;
/// How far either side of the wanted station the others can land, in hertz.
/// The floor is above one critical band of hearing for a reason — see
/// [`crate::timing::PILEUP_MIN_SEPARATION_HZ`].
pub const PILEUP_SPREAD_MIN: f64 = 150.0;
pub const PILEUP_SPREAD_MAX: f64 = 800.0;
/// How far below the wanted station the others sit, in decibels. The floor is
/// what keeps it answerable: the one you want stays the strongest. It is not
/// far above zero because a station much more than ten decibels down, this
/// close in pitch, is simply masked rather than quiet.
pub const PILEUP_LEVEL_MIN_DB: f64 = 3.0;
pub const PILEUP_LEVEL_MAX_DB: f64 = 20.0;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct BandSettings {
    pub side_tone_min: f64,
    pub side_tone_max: f64,
    pub volume_min: f64,
    pub volume_max: f64,
    pub link_volume: bool,
    pub steepness: f64,
    pub envelope_smoothing: f64,
    #[serde(default = "defaults::enabled")]
    pub qsb_enabled: bool,
    #[serde(default = "defaults::qsb_depth")]
    pub qsb_depth: f64,
    #[serde(default = "defaults::qsb_rate_hz")]
    pub qsb_rate_hz: f64,
    #[serde(default = "defaults::enabled")]
    pub qrn_enabled: bool,
    #[serde(default = "defaults::qrn_level")]
    pub qrn_level: f64,
    // These three were called `qrm*` before the name went to the stations it
    // belongs to. The aliases are what keep every save written until now.
    #[serde(default = "defaults::enabled", alias = "qrmEnabled")]
    pub receiver_enabled: bool,
    #[serde(default = "defaults::receiver_level", alias = "qrmLevel")]
    pub receiver_level: f64,
    #[serde(default, alias = "qrmProfile")]
    pub receiver_profile: ReceiverProfile,
    #[serde(default = "defaults::receiver_background_gain")]
    pub receiver_background_gain: f64,
    #[serde(default = "defaults::receiver_background_excitation_rate")]
    pub receiver_background_excitation_rate: f64,
    /// The receiver's selectivity, in hertz. Everything you hear goes through
    /// it — the Morse as much as the noise — so narrowing it does what
    /// narrowing a real filter does: less static gets through, the signal
    /// loses a little of its keying sidebands, and the filter rings longer.
    #[serde(default = "defaults::filter_bandwidth_hz")]
    pub filter_bandwidth_hz: f64,
    /// The fewest stations that can call at once, counting the one you want.
    ///
    /// Saves written before the pile-up became a range said only "up to this
    /// many", which is this floor left at one.
    #[serde(default = "defaults::stations_min")]
    pub stations_min: u32,
    /// The most that can call at once. With both ends at 1 there is no
    /// pile-up, which is where this starts.
    #[serde(default = "defaults::stations_max")]
    pub stations_max: u32,
    /// How far either side of the wanted station the others can land.
    #[serde(default = "defaults::pileup_spread_hz")]
    pub pileup_spread_hz: f64,
    /// How far below the wanted station the others sit.
    #[serde(default = "defaults::pileup_level_db")]
    pub pileup_level_db: f64,
    #[serde(default = "defaults::receiver_background_resonance")]
    pub receiver_background_resonance: f64,
    #[serde(default = "defaults::receiver_background_decay")]
    pub receiver_background_decay: f64,
    #[serde(default = "defaults::receiver_background_offset_hz")]
    pub receiver_background_offset_hz: f64,
    #[serde(default = "defaults::receiver_background_offset_mod_depth_hz")]
    pub receiver_background_offset_mod_depth_hz: f64,
    #[serde(default = "defaults::receiver_background_offset_mod_rate_hz")]
    pub receiver_background_offset_mod_rate_hz: f64,
}

impl Default for BandSettings {
    fn default() -> Self {
        Self {
            side_tone_min: 400.0,
            side_tone_max: 600.0,
            volume_min: 0.7,
            volume_max: 1.0,
            link_volume: false,
            steepness: 10.0,
            envelope_smoothing: 0.75,
            qsb_enabled: true,
            qsb_depth: 0.35,
            qsb_rate_hz: 0.12,
            qrn_enabled: true,
            qrn_level: 0.25,
            receiver_enabled: true,
            receiver_level: 0.2,
            receiver_profile: ReceiverProfile::Mixed,
            receiver_background_gain: 20.0,
            receiver_background_excitation_rate: 62.0,
            filter_bandwidth_hz: 500.0,
            stations_min: STATIONS_MIN,
            stations_max: STATIONS_MIN,
            pileup_spread_hz: 350.0,
            pileup_level_db: 7.0,
            receiver_background_resonance: 66.0,
            receiver_background_decay: 0.984,
            receiver_background_offset_hz: 140.0,
            receiver_background_offset_mod_depth_hz: 45.0,
            receiver_background_offset_mod_rate_hz: 0.32,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AutoLevelSettings {
    pub mixed_auto_level_next_axis: MixedAutoLevelAxis,
    #[serde(alias = "autoAdjustKoch", default = "defaults::enabled")]
    pub auto_adjust_level: bool,
    pub auto_adjust_threshold: f64,
    pub auto_adjust_below_threshold_count: u32,
    pub auto_adjust_above_threshold_count: u32,
    pub error_weight_strength: f64,
    pub char_sampling_coverage_strength: f64,
    #[serde(default)]
    pub char_sampling_thompson: bool,
}

impl Default for AutoLevelSettings {
    fn default() -> Self {
        Self {
            mixed_auto_level_next_axis: MixedAutoLevelAxis::Letters,
            auto_adjust_level: true,
            auto_adjust_threshold: 90.0,
            auto_adjust_below_threshold_count: 1,
            auto_adjust_above_threshold_count: 5,
            error_weight_strength: 3.0,
            char_sampling_coverage_strength: 1.0,
            char_sampling_thompson: false,
        }
    }
}

/// The min/max pairs the settings screen edits. Keeping the invariants here —
/// max never below min, "linked" collapsing the pair, character speed dragging
/// effective speed along — keeps them testable and out of the UI's callbacks.
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
    /// Character and effective speed, and the spacing between words.
    Speed,
    /// Attack and decay of the keying.
    KeyingEnvelope,
    /// Which characters are practised, and how far into them you are.
    CharacterSet,
    /// Pitch and level of the station you are copying.
    ToneAndVolume,
    /// Fading, static, the filter and who else is calling.
    BandConditions,
    /// The advanced receiver-character model behind the background.
    ReceiverModel,
    /// Whether the trainer moves your level for you, and on what evidence.
    AutoLevel,
    /// How the next group is drawn from the characters you know.
    CharacterSampling,
}

impl SettingsSection {
    /// Every section, for the tests and for anything that wants to sweep them.
    pub const ALL: [Self; 9] = [
        Self::SessionShape,
        Self::Speed,
        Self::KeyingEnvelope,
        Self::CharacterSet,
        Self::ToneAndVolume,
        Self::BandConditions,
        Self::ReceiverModel,
        Self::AutoLevel,
        Self::CharacterSampling,
    ];
}

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

/// How far apart the side tone bounds are pushed when a fixed pitch is opened
/// back up into a range.
const SIDE_TONE_SPREAD_HZ: f64 = 200.0;
const SIDE_TONE_MIN_HZ: f64 = 200.0;
const SIDE_TONE_MAX_HZ: f64 = 1200.0;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default = "TrainingSettings::default")]
pub struct TrainingSettings {
    #[serde(flatten)]
    pub curriculum: CurriculumSettings,
    #[serde(flatten)]
    pub playback: PlaybackSettings,
    #[serde(flatten)]
    pub band: BandSettings,
    #[serde(flatten)]
    pub auto_level: AutoLevelSettings,
}

impl Default for TrainingSettings {
    fn default() -> Self {
        Self {
            curriculum: CurriculumSettings::default(),
            playback: PlaybackSettings::default(),
            band: BandSettings::default(),
            auto_level: AutoLevelSettings::default(),
        }
    }
}

impl TrainingSettings {
    /// Unique Morse characters, first-seen order, skipping whitespace and unknowns.
    pub fn unique_alphabet(chars: &[char]) -> Vec<char> {
        let mut out = Vec::new();
        for ch in chars {
            let up = ch.to_ascii_uppercase();
            if up.is_whitespace() || morse_for(up).is_none() {
                continue;
            }
            if !out.contains(&up) {
                out.push(up);
            }
        }
        out
    }

    /// Ordered alphabet that `level` unlocks. Custom uses `custom_set` when set.
    /// Mixed drops digits so the letter axis and digits axis stay separate.
    pub fn progress_alphabet(&self) -> Vec<char> {
        let base = if self.curriculum.char_set_mode == CharSetMode::Custom {
            let custom = Self::unique_alphabet(&self.curriculum.custom_set);
            if custom.is_empty() {
                Self::unique_alphabet(self.sequence())
            } else {
                custom
            }
        } else {
            Self::unique_alphabet(self.sequence())
        };
        if self.curriculum.char_set_mode == CharSetMode::Mixed {
            base.into_iter().filter(|c| !is_digit(*c)).collect()
        } else {
            base
        }
    }

    pub fn max_letter_level(&self) -> u32 {
        max_level_for_len(self.progress_alphabet().len())
    }

    pub fn active_alphabet(&self) -> Vec<char> {
        match self.curriculum.char_set_mode {
            CharSetMode::Digits => crate::morse::DIGITS.to_vec(),
            CharSetMode::Callsign => crate::callsign::callsign_pool(self.curriculum.callsign_level),
            _ => self.progress_alphabet(),
        }
    }

    pub fn active_level(&self) -> u32 {
        match self.curriculum.char_set_mode {
            CharSetMode::Digits => self.curriculum.digits_level,
            CharSetMode::Callsign => self.curriculum.callsign_level,
            _ => self.curriculum.level,
        }
    }

    pub fn set_active_level(&mut self, value: u32) {
        match self.curriculum.char_set_mode {
            CharSetMode::Digits => self.curriculum.digits_level = value,
            CharSetMode::Callsign => self.curriculum.callsign_level = value,
            _ => self.curriculum.level = value,
        }
    }

    pub fn max_active_level(&self) -> u32 {
        match self.curriculum.char_set_mode {
            CharSetMode::Digits => max_level_for_len(crate::morse::DIGITS.len()),
            CharSetMode::Callsign => crate::callsign::CALLSIGN_TIER_MAX,
            _ => self.max_letter_level(),
        }
    }

    pub fn clamp(mut self) -> Self {
        let seq_max = self.max_letter_level();
        self.curriculum.level = self.curriculum.level.clamp(LEVEL_MIN, seq_max);
        self.curriculum.digits_level = self
            .curriculum
            .digits_level
            .clamp(LEVEL_MIN, max_level_for_len(crate::morse::DIGITS.len()));
        self.curriculum.callsign_level = self.curriculum.callsign_level.clamp(
            crate::callsign::CALLSIGN_TIER_MIN,
            crate::callsign::CALLSIGN_TIER_MAX,
        );
        self.curriculum.mixed_letters_percent = self.curriculum.mixed_letters_percent.min(100);
        self.curriculum.num_groups = self.curriculum.num_groups.clamp(1, 200);
        self.curriculum.min_group_size = self.curriculum.min_group_size.clamp(1, 20);
        self.curriculum.max_group_size = self
            .curriculum
            .max_group_size
            .clamp(self.curriculum.min_group_size, 20);
        if self.curriculum.link_group_size {
            self.curriculum.max_group_size = self.curriculum.min_group_size;
        }
        self.playback.char_wpm_min = self.playback.char_wpm_min.clamp(5.0, 80.0);
        self.playback.char_wpm_max = self
            .playback
            .char_wpm_max
            .clamp(self.playback.char_wpm_min, 80.0);
        if self.playback.link_char_wpm {
            self.playback.char_wpm_max = self.playback.char_wpm_min;
        }
        self.playback.effective_wpm_min = self.playback.effective_wpm_min.clamp(5.0, 80.0);
        self.playback.effective_wpm_max = self
            .playback
            .effective_wpm_max
            .clamp(self.playback.effective_wpm_min, 80.0);
        if self.playback.link_effective_wpm {
            self.playback.effective_wpm_max = self.playback.effective_wpm_min;
        }
        if self.playback.link_char_to_effective {
            self.playback.effective_wpm_min = self.playback.char_wpm_min;
            self.playback.effective_wpm_max = self.playback.char_wpm_max;
        }
        self.band.filter_bandwidth_hz = self
            .band
            .filter_bandwidth_hz
            .clamp(FILTER_BANDWIDTH_MIN, FILTER_BANDWIDTH_MAX);
        self.band.stations_min = self.band.stations_min.clamp(STATIONS_MIN, STATIONS_MAX);
        self.band.stations_max = self
            .band
            .stations_max
            .clamp(self.band.stations_min, STATIONS_MAX);
        self.band.pileup_spread_hz = self
            .band
            .pileup_spread_hz
            .clamp(PILEUP_SPREAD_MIN, PILEUP_SPREAD_MAX);
        self.band.pileup_level_db = self
            .band
            .pileup_level_db
            .clamp(PILEUP_LEVEL_MIN_DB, PILEUP_LEVEL_MAX_DB);
        self.band.side_tone_min = self.band.side_tone_min.clamp(200.0, 1200.0);
        self.band.side_tone_max = self
            .band
            .side_tone_max
            .clamp(self.band.side_tone_min, 1200.0);
        self.band.volume_min = self.band.volume_min.clamp(0.1, 1.0);
        self.band.volume_max = self.band.volume_max.clamp(self.band.volume_min, 1.0);
        if self.band.link_volume {
            self.band.volume_max = self.band.volume_min;
        }
        self.band.steepness = self.band.steepness.clamp(1.0, 50.0);
        self.band.envelope_smoothing = self.band.envelope_smoothing.clamp(0.0, 1.0);
        self.band.qsb_depth = self.band.qsb_depth.clamp(0.0, 1.0);
        self.band.qsb_rate_hz = self.band.qsb_rate_hz.clamp(0.03, 1.5);
        self.band.qrn_level = self.band.qrn_level.clamp(0.0, 1.0);
        self.band.receiver_level = self.band.receiver_level.clamp(0.0, 1.0);
        self.band.receiver_background_gain = self
            .band
            .receiver_background_gain
            .clamp(0.0, RECEIVER_MODEL_GAIN_MAX);
        self.band.receiver_background_excitation_rate = self
            .band
            .receiver_background_excitation_rate
            .clamp(0.1, 500.0);
        self.band.receiver_background_resonance =
            self.band.receiver_background_resonance.clamp(0.5, 240.0);
        self.band.receiver_background_decay =
            self.band.receiver_background_decay.clamp(0.5, 0.9999);
        self.band.receiver_background_offset_hz = self
            .band
            .receiver_background_offset_hz
            .clamp(-1000.0, 1000.0);
        self.band.receiver_background_offset_mod_depth_hz = self
            .band
            .receiver_background_offset_mod_depth_hz
            .clamp(0.0, 1000.0);
        self.band.receiver_background_offset_mod_rate_hz = self
            .band
            .receiver_background_offset_mod_rate_hz
            .clamp(0.0, 20.0);
        self.playback.group_repeat_min = self
            .playback
            .group_repeat_min
            .clamp(GROUP_REPEAT_MIN, GROUP_REPEAT_MAX);
        self.playback.group_repeat_max = self
            .playback
            .group_repeat_max
            .clamp(self.playback.group_repeat_min, GROUP_REPEAT_MAX);
        if self.playback.link_group_repeat {
            self.playback.group_repeat_max = self.playback.group_repeat_min;
        }
        self.playback.extra_word_space_multiplier =
            self.playback.extra_word_space_multiplier.max(0.1);
        self.playback.group_timeout = self.playback.group_timeout.clamp(0.0, 120.0);
        self.auto_level.auto_adjust_threshold =
            self.auto_level.auto_adjust_threshold.clamp(0.0, 100.0);
        self.auto_level.error_weight_strength = self.auto_level.error_weight_strength.max(0.0);
        self.auto_level.char_sampling_coverage_strength =
            self.auto_level.char_sampling_coverage_strength.max(0.0);
        self
    }

    pub fn sequence(&self) -> &[char] {
        if self.curriculum.custom_sequence.is_empty() {
            crate::morse::LCWO_SEQUENCE
        } else {
            &self.curriculum.custom_sequence
        }
    }

    /// Identity of the alphabet this mode is training, used to isolate auto-level
    /// counters and sampling history when the user switches sequence or custom set.
    pub fn alphabet_fingerprint(&self) -> String {
        match self.curriculum.char_set_mode {
            CharSetMode::Digits => crate::morse::DIGITS.iter().copied().collect(),
            // One fingerprint for every tier: the characters do not change as
            // you climb, so what you learned at tier 1 still counts at tier 6.
            CharSetMode::Callsign => "callsign".to_string(),
            _ => self.progress_alphabet().into_iter().collect(),
        }
    }

    /// Put one card back the way it shipped, leaving every other card, and all
    /// of your progress, alone.
    ///
    /// Values come from `Self::default()` rather than being written out again
    /// here, so a changed default reaches the reset button on its own.
    pub fn reset_section(&mut self, which: SettingsSection) {
        let d = Self::default();
        match which {
            SettingsSection::SessionShape => {
                self.curriculum.num_groups = d.curriculum.num_groups;
                self.curriculum.min_group_size = d.curriculum.min_group_size;
                self.curriculum.max_group_size = d.curriculum.max_group_size;
                self.curriculum.link_group_size = d.curriculum.link_group_size;
                self.playback.group_repeat_min = d.playback.group_repeat_min;
                self.playback.group_repeat_max = d.playback.group_repeat_max;
                self.playback.link_group_repeat = d.playback.link_group_repeat;
                self.playback.group_timeout = d.playback.group_timeout;
                self.playback.lock_input_during_group_playback =
                    d.playback.lock_input_during_group_playback;
            }
            SettingsSection::Speed => {
                self.playback.char_wpm_min = d.playback.char_wpm_min;
                self.playback.char_wpm_max = d.playback.char_wpm_max;
                self.playback.link_char_wpm = d.playback.link_char_wpm;
                self.playback.effective_wpm_min = d.playback.effective_wpm_min;
                self.playback.effective_wpm_max = d.playback.effective_wpm_max;
                self.playback.link_effective_wpm = d.playback.link_effective_wpm;
                self.playback.link_char_to_effective = d.playback.link_char_to_effective;
                self.playback.extra_word_space_multiplier = d.playback.extra_word_space_multiplier;
            }
            SettingsSection::KeyingEnvelope => {
                self.band.steepness = d.band.steepness;
                self.band.envelope_smoothing = d.band.envelope_smoothing;
            }
            SettingsSection::CharacterSet => {
                self.curriculum.char_set_mode = d.curriculum.char_set_mode;
                self.curriculum.mixed_letters_percent = d.curriculum.mixed_letters_percent;
                self.curriculum.custom_set = d.curriculum.custom_set.clone();
                self.curriculum.custom_sequence = d.curriculum.custom_sequence.clone();
                self.curriculum.sequence_is_custom = d.curriculum.sequence_is_custom;
                self.curriculum.practice_window = d.curriculum.practice_window;
                self.curriculum.sliding_window_start = d.curriculum.sliding_window_start;
                self.curriculum.sliding_window_end = d.curriculum.sliding_window_end;
            }
            SettingsSection::ToneAndVolume => {
                self.band.side_tone_min = d.band.side_tone_min;
                self.band.side_tone_max = d.band.side_tone_max;
                self.band.volume_min = d.band.volume_min;
                self.band.volume_max = d.band.volume_max;
                self.band.link_volume = d.band.link_volume;
            }
            SettingsSection::BandConditions => {
                self.band.qsb_enabled = d.band.qsb_enabled;
                self.band.qsb_depth = d.band.qsb_depth;
                self.band.qsb_rate_hz = d.band.qsb_rate_hz;
                self.band.qrn_enabled = d.band.qrn_enabled;
                self.band.qrn_level = d.band.qrn_level;
                self.band.receiver_enabled = d.band.receiver_enabled;
                self.band.receiver_level = d.band.receiver_level;
                self.band.receiver_profile = d.band.receiver_profile;
                self.band.filter_bandwidth_hz = d.band.filter_bandwidth_hz;
                self.band.stations_min = d.band.stations_min;
                self.band.stations_max = d.band.stations_max;
                self.band.pileup_spread_hz = d.band.pileup_spread_hz;
                self.band.pileup_level_db = d.band.pileup_level_db;
            }
            SettingsSection::ReceiverModel => {
                self.band.receiver_background_gain = d.band.receiver_background_gain;
                self.band.receiver_background_excitation_rate =
                    d.band.receiver_background_excitation_rate;
                self.band.receiver_background_resonance = d.band.receiver_background_resonance;
                self.band.receiver_background_decay = d.band.receiver_background_decay;
                self.band.receiver_background_offset_hz = d.band.receiver_background_offset_hz;
                self.band.receiver_background_offset_mod_depth_hz =
                    d.band.receiver_background_offset_mod_depth_hz;
                self.band.receiver_background_offset_mod_rate_hz =
                    d.band.receiver_background_offset_mod_rate_hz;
            }
            SettingsSection::AutoLevel => {
                self.auto_level.auto_adjust_level = d.auto_level.auto_adjust_level;
                self.auto_level.auto_adjust_threshold = d.auto_level.auto_adjust_threshold;
                self.auto_level.auto_adjust_below_threshold_count =
                    d.auto_level.auto_adjust_below_threshold_count;
                self.auto_level.auto_adjust_above_threshold_count =
                    d.auto_level.auto_adjust_above_threshold_count;
                self.auto_level.mixed_auto_level_next_axis =
                    d.auto_level.mixed_auto_level_next_axis;
            }
            SettingsSection::CharacterSampling => {
                self.auto_level.error_weight_strength = d.auto_level.error_weight_strength;
                self.auto_level.char_sampling_coverage_strength =
                    d.auto_level.char_sampling_coverage_strength;
                self.auto_level.char_sampling_thompson = d.auto_level.char_sampling_thompson;
            }
        }
    }

    pub fn range(&self, which: RangeSetting) -> RangeValues {
        let (min, max, linked) = match which {
            RangeSetting::CharWpm => (
                self.playback.char_wpm_min,
                self.playback.char_wpm_max,
                self.playback.link_char_wpm,
            ),
            RangeSetting::EffectiveWpm => (
                self.playback.effective_wpm_min,
                self.playback.effective_wpm_max,
                self.playback.link_effective_wpm,
            ),
            RangeSetting::GroupSize => (
                f64::from(self.curriculum.min_group_size),
                f64::from(self.curriculum.max_group_size),
                self.curriculum.link_group_size,
            ),
            RangeSetting::GroupRepeat => (
                f64::from(self.playback.group_repeat_min),
                f64::from(self.playback.group_repeat_max),
                self.playback.link_group_repeat,
            ),
            // The side tone has no stored link flag: a single pitch is simply
            // a range with both ends together.
            RangeSetting::SideTone => (
                self.band.side_tone_min,
                self.band.side_tone_max,
                (self.band.side_tone_min - self.band.side_tone_max).abs() < f64::EPSILON,
            ),
            RangeSetting::Volume => (
                self.band.volume_min,
                self.band.volume_max,
                self.band.link_volume,
            ),
            // Like the side tone, the bounds themselves say whether the count
            // is fixed — there is no separate flag to keep in step with them.
            RangeSetting::Stations => (
                f64::from(self.band.stations_min),
                f64::from(self.band.stations_max),
                self.band.stations_min == self.band.stations_max,
            ),
        };
        RangeValues { min, max, linked }
    }

    /// Move the lower bound. The upper bound follows when it would end up below
    /// it, or when the pair is linked.
    pub fn set_range_min(&mut self, which: RangeSetting, value: f64) {
        let current = self.range(which);
        let max = if current.linked || value > current.max {
            value
        } else {
            current.max
        };
        self.write_range(which, value, max);
    }

    /// Move the upper bound, dragging the lower one down if it would overtake it.
    pub fn set_range_max(&mut self, which: RangeSetting, value: f64) {
        let current = self.range(which);
        let min = if current.linked || value < current.min {
            value
        } else {
            current.min
        };
        self.write_range(which, min, value);
    }

    /// Collapse a range to its lower bound, or open a fixed value back up.
    pub fn set_range_linked(&mut self, which: RangeSetting, linked: bool) {
        let current = self.range(which);
        match which {
            RangeSetting::CharWpm => self.playback.link_char_wpm = linked,
            RangeSetting::EffectiveWpm => self.playback.link_effective_wpm = linked,
            RangeSetting::GroupSize => self.curriculum.link_group_size = linked,
            RangeSetting::GroupRepeat => self.playback.link_group_repeat = linked,
            // Nothing to store: the bounds themselves say whether it is fixed.
            RangeSetting::SideTone | RangeSetting::Stations => {}
            RangeSetting::Volume => self.band.link_volume = linked,
        }
        if linked {
            self.write_range(which, current.min, current.min);
        } else if which == RangeSetting::SideTone {
            let max = (current.min + SIDE_TONE_SPREAD_HZ).min(SIDE_TONE_MAX_HZ);
            let min = (max - SIDE_TONE_SPREAD_HZ).max(SIDE_TONE_MIN_HZ);
            self.write_range(which, min, max);
        } else if which == RangeSetting::Stations {
            // Neither of these stores a link flag — the bounds themselves say
            // whether the value is fixed — so opening one back up has to push
            // them apart, or the button would do nothing at all. One station
            // either side is the smallest range that is still a range.
            let max = (current.min + 1.0).min(f64::from(STATIONS_MAX));
            let min = (max - 1.0).max(f64::from(STATIONS_MIN));
            self.write_range(which, min, max);
        }
    }

    fn write_range(&mut self, which: RangeSetting, min: f64, max: f64) {
        let as_u32 = |value: f64| value.max(0.0).round() as u32;
        match which {
            RangeSetting::CharWpm => {
                self.playback.char_wpm_min = min;
                self.playback.char_wpm_max = max;
                self.sync_effective_to_char();
            }
            RangeSetting::EffectiveWpm => {
                self.playback.effective_wpm_min = min;
                self.playback.effective_wpm_max = max;
            }
            RangeSetting::GroupSize => {
                self.curriculum.min_group_size = as_u32(min);
                self.curriculum.max_group_size = as_u32(max);
            }
            RangeSetting::GroupRepeat => {
                self.playback.group_repeat_min = as_u32(min);
                self.playback.group_repeat_max = as_u32(max);
            }
            RangeSetting::SideTone => {
                self.band.side_tone_min = min;
                self.band.side_tone_max = max;
            }
            RangeSetting::Volume => {
                self.band.volume_min = min;
                self.band.volume_max = max;
            }
            RangeSetting::Stations => {
                self.band.stations_min = as_u32(min);
                self.band.stations_max = as_u32(max);
            }
        }
    }

    /// Farnsworth off: effective speed simply mirrors character speed.
    pub fn sync_effective_to_char(&mut self) {
        if self.playback.link_char_to_effective {
            self.playback.effective_wpm_min = self.playback.char_wpm_min;
            self.playback.effective_wpm_max = self.playback.char_wpm_max;
        }
    }

    /// Switch character set, resetting the practice window with it.
    pub fn set_char_set_mode(&mut self, mode: CharSetMode) {
        self.curriculum.char_set_mode = mode;
        self.curriculum.practice_window = Some(PracticeWindow::All);
    }

    pub fn side_tone_center(&self) -> f64 {
        let min = self.band.side_tone_min.max(100.0);
        let max = self.band.side_tone_max.max(min);
        min + (max - min) / 2.0
    }

    pub fn band_signature(&self) -> String {
        format!(
            "{}|{}|{}|{}|{}|{}|{}|{}|{}|{:?}|{}|{}|{}|{}|{}|{}|{}|{}",
            self.band.side_tone_min,
            self.band.side_tone_max,
            self.band.qsb_enabled,
            self.band.qsb_depth,
            self.band.qsb_rate_hz,
            self.band.qrn_enabled,
            self.band.qrn_level,
            self.band.receiver_enabled,
            self.band.receiver_level,
            self.band.receiver_profile,
            self.band.receiver_background_gain,
            self.band.receiver_background_excitation_rate,
            self.band.receiver_background_resonance,
            self.band.receiver_background_decay,
            self.band.receiver_background_offset_hz,
            self.band.receiver_background_offset_mod_depth_hz,
            self.band.receiver_background_offset_mod_rate_hz,
            self.band.filter_bandwidth_hz,
        )
    }
}

mod defaults {
    pub fn enabled() -> bool {
        true
    }
    pub fn group_repeat() -> u32 {
        1
    }
    pub fn level() -> u32 {
        crate::level::LEVEL_MIN
    }
    pub fn qsb_depth() -> f64 {
        0.35
    }
    pub fn qsb_rate_hz() -> f64 {
        0.12
    }
    pub fn qrn_level() -> f64 {
        0.25
    }
    pub fn receiver_level() -> f64 {
        0.2
    }
    pub fn receiver_background_gain() -> f64 {
        20.0
    }
    pub fn receiver_background_excitation_rate() -> f64 {
        62.0
    }
    pub fn receiver_background_resonance() -> f64 {
        66.0
    }
    pub fn filter_bandwidth_hz() -> f64 {
        500.0
    }
    pub fn stations_max() -> u32 {
        super::STATIONS_MIN
    }
    pub fn stations_min() -> u32 {
        super::STATIONS_MIN
    }
    pub fn pileup_spread_hz() -> f64 {
        350.0
    }
    pub fn pileup_level_db() -> f64 {
        7.0
    }
    pub fn receiver_background_decay() -> f64 {
        0.984
    }
    pub fn receiver_background_offset_hz() -> f64 {
        140.0
    }
    pub fn receiver_background_offset_mod_depth_hz() -> f64 {
        45.0
    }
    pub fn receiver_background_offset_mod_rate_hz() -> f64 {
        0.32
    }
}

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
        assert_eq!(s.curriculum.level, s.max_letter_level());
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
    fn character_speed_drags_effective_speed_when_tied() {
        let mut s = TrainingSettings::default();
        s.playback.link_char_to_effective = true;
        s.playback.link_char_wpm = false;
        s.set_range_min(RangeSetting::CharWpm, 15.0);
        s.set_range_max(RangeSetting::CharWpm, 28.0);
        assert_eq!(s.range(RangeSetting::EffectiveWpm).min, 15.0);
        assert_eq!(s.range(RangeSetting::EffectiveWpm).max, 28.0);

        // Untied, effective speed is independent again.
        s.playback.link_char_to_effective = false;
        s.set_range_min(RangeSetting::EffectiveWpm, 9.0);
        s.set_range_max(RangeSetting::CharWpm, 30.0);
        assert_eq!(s.range(RangeSetting::EffectiveWpm).min, 9.0);
    }

    #[test]
    fn the_side_tone_is_fixed_when_both_ends_match() {
        let mut s = TrainingSettings::default();
        assert!(!s.range(RangeSetting::SideTone).linked);

        s.set_range_linked(RangeSetting::SideTone, true);
        let range = s.range(RangeSetting::SideTone);
        assert!(range.linked);
        assert_eq!(range.min, range.max);

        // Opening it back up spreads the bounds without leaving the band.
        s.set_range_linked(RangeSetting::SideTone, false);
        let range = s.range(RangeSetting::SideTone);
        assert!(!range.linked);
        assert_eq!(range.max - range.min, 200.0);
        assert!(range.min >= 200.0 && range.max <= 1200.0);

        // Even at the top of the band the spread stays inside it.
        s.band.side_tone_min = 1200.0;
        s.band.side_tone_max = 1200.0;
        s.set_range_linked(RangeSetting::SideTone, false);
        let range = s.range(RangeSetting::SideTone);
        assert_eq!((range.min, range.max), (1000.0, 1200.0));
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

    #[test]
    fn switching_mode_resets_the_practice_window() {
        let mut s = TrainingSettings::default();
        s.curriculum.practice_window = Some(PracticeWindow::Last3);
        s.set_char_set_mode(CharSetMode::Digits);
        assert_eq!(s.curriculum.char_set_mode, CharSetMode::Digits);
        assert_eq!(s.curriculum.practice_window, Some(PracticeWindow::All));
    }

    #[test]
    fn custom_level_clamps_to_alphabet_length() {
        let mut s = TrainingSettings::default();
        s.curriculum.char_set_mode = CharSetMode::Custom;
        s.curriculum.custom_set = vec!['A', 'B', 'C', 'D'];
        s.curriculum.level = 99;
        let s = s.clamp();
        assert_eq!(s.curriculum.level, 3);
        assert_eq!(s.progress_alphabet(), vec!['A', 'B', 'C', 'D']);
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
        assert_eq!(s.auto_level.auto_adjust_threshold, 90.0);
        assert_eq!(s.curriculum.num_groups, 20);
        assert_eq!(s.curriculum.digits_level, 1);
        assert_eq!(s.playback.char_wpm_min, 18.0);
        assert_eq!(s.playback.group_repeat_min, 1);
        assert_eq!(s.playback.group_repeat_max, 1);
        assert!(s.playback.link_group_repeat);
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
}

#[cfg(test)]
mod invariant_tests {
    use super::*;
    use crate::pool::fit_settings_to_alphabet;

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
        s.curriculum.custom_set = vec!['#'];
        s.curriculum.sliding_window_start = 900;
        s.curriculum.sliding_window_end = 0;
        s.playback.char_wpm_min = 900.0;
        s.playback.char_wpm_max = -5.0;
        s.playback.effective_wpm_min = -1.0;
        s.playback.effective_wpm_max = 900.0;
        s.playback.extra_word_space_multiplier = -3.0;
        s.playback.group_timeout = 9_000.0;
        s.playback.group_repeat_min = 99;
        s.playback.group_repeat_max = 0;
        s.band.side_tone_min = 9_000.0;
        s.band.side_tone_max = 1.0;
        s.band.volume_min = 9.0;
        s.band.volume_max = -9.0;
        s.band.steepness = 900.0;
        s.band.envelope_smoothing = 9.0;
        s.band.qsb_depth = 9.0;
        s.band.qsb_rate_hz = 9.0;
        s.band.qrn_level = 9.0;
        s.band.receiver_level = 9.0;
        s.band.receiver_background_gain = 900.0;
        s.band.receiver_background_excitation_rate = 9_000.0;
        s.band.receiver_background_resonance = 9_000.0;
        s.band.receiver_background_decay = 9.0;
        s.band.receiver_background_offset_hz = -9_000.0;
        s.band.receiver_background_offset_mod_depth_hz = 9_000.0;
        s.band.receiver_background_offset_mod_rate_hz = 900.0;
        s.auto_level.auto_adjust_threshold = 900.0;
        s.auto_level.error_weight_strength = -1.0;
        s.auto_level.char_sampling_coverage_strength = -1.0;
        s
    }

    /// The app re-runs clamp + fit on every settings change and writes the
    /// result back into the same signal. A second pass that moved anything
    /// would make that effect loop forever.
    #[test]
    fn clamping_and_fitting_reach_a_fixed_point_in_one_pass() {
        for mode in [
            CharSetMode::Koch,
            CharSetMode::Digits,
            CharSetMode::Mixed,
            CharSetMode::Custom,
        ] {
            for linked in [false, true] {
                let mut first = wild();
                first.curriculum.char_set_mode = mode;
                first.curriculum.link_group_size = linked;
                first.playback.link_char_wpm = linked;
                first.playback.link_effective_wpm = linked;
                first.playback.link_char_to_effective = linked;
                first.playback.link_group_repeat = linked;
                first.band.link_volume = linked;
                let mut first = first.clamp();
                fit_settings_to_alphabet(&mut first);
                let mut second = first.clone().clamp();
                fit_settings_to_alphabet(&mut second);
                assert_eq!(first, second, "mode {mode:?}, linked {linked}");
            }
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
        assert_eq!(s.playback.group_repeat_max, GROUP_REPEAT_MAX);
        assert_eq!(s.band.steepness, 50.0);
        assert_eq!(s.band.envelope_smoothing, 1.0);
        assert_eq!(s.band.receiver_background_decay, 0.9999);
        assert_eq!(s.band.receiver_background_offset_hz, -1000.0);
        assert_eq!(s.band.receiver_background_offset_mod_rate_hz, 20.0);
        assert_eq!(s.band.receiver_background_excitation_rate, 500.0);
        assert_eq!(s.playback.extra_word_space_multiplier, 0.1);
        assert_eq!(s.auto_level.auto_adjust_threshold, 100.0);
        assert_eq!(s.auto_level.error_weight_strength, 0.0);
        assert_eq!(s.auto_level.char_sampling_coverage_strength, 0.0);
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

    #[test]
    fn defaults_are_the_documented_ones() {
        assert_eq!(CharSetMode::default(), CharSetMode::Mixed);
        assert_eq!(ReceiverProfile::default(), ReceiverProfile::Mixed);
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
        assert_eq!(s.active_alphabet(), crate::morse::DIGITS.to_vec());
        assert_eq!(s.max_active_level(), 9);
        s.set_active_level(3);
        assert_eq!(s.curriculum.digits_level, 3);
        assert_eq!(s.curriculum.level, 4);

        s.curriculum.char_set_mode = CharSetMode::Koch;
        assert_eq!(s.active_level(), 4);
        assert_eq!(s.max_active_level(), s.max_letter_level());
        assert_eq!(s.active_alphabet(), s.progress_alphabet());
        s.set_active_level(7);
        assert_eq!(s.curriculum.level, 7);
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
    fn the_band_signature_changes_with_every_band_setting() {
        let base = TrainingSettings::default();
        let mut seen = vec![base.band_signature()];
        let edits: Vec<fn(&mut TrainingSettings)> = vec![
            |s| s.band.side_tone_min = 333.0,
            |s| s.band.side_tone_max = 999.0,
            |s| s.band.qsb_enabled = false,
            |s| s.band.qsb_depth = 0.9,
            |s| s.band.qsb_rate_hz = 0.9,
            |s| s.band.qrn_enabled = false,
            |s| s.band.qrn_level = 0.9,
            |s| s.band.receiver_enabled = false,
            |s| s.band.receiver_level = 0.9,
            |s| s.band.receiver_profile = ReceiverProfile::Ringing,
            |s| s.band.receiver_background_gain = 3.0,
            |s| s.band.receiver_background_excitation_rate = 3.0,
            |s| s.band.receiver_background_resonance = 3.0,
            |s| s.band.receiver_background_decay = 0.7,
            |s| s.band.receiver_background_offset_hz = 3.0,
            |s| s.band.receiver_background_offset_mod_depth_hz = 3.0,
            |s| s.band.receiver_background_offset_mod_rate_hz = 3.0,
        ];
        for edit in edits {
            let mut changed = base.clone();
            edit(&mut changed);
            let signature = changed.band_signature();
            assert!(
                !seen.contains(&signature),
                "signature repeated: {signature}"
            );
            seen.push(signature);
        }
        // Speed is not a band setting, so it leaves the signature alone.
        let mut faster = base.clone();
        faster.playback.char_wpm_min = 40.0;
        assert_eq!(faster.band_signature(), base.band_signature());
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
        s.curriculum.practice_window = Some(PracticeWindow::Last3);
        s.set_char_set_mode(CharSetMode::Digits);
        assert_eq!(s.curriculum.char_set_mode, CharSetMode::Digits);
        assert_eq!(s.curriculum.practice_window, Some(PracticeWindow::All));
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

    /// Settings with every stored preference moved off its default, so a reset
    /// has something to undo whichever section it is asked about.
    fn wildly_different() -> TrainingSettings {
        let mut s = TrainingSettings::default();
        s.curriculum.num_groups = 7;
        s.curriculum.min_group_size = 2;
        s.curriculum.max_group_size = 9;
        s.curriculum.link_group_size = true;
        s.curriculum.char_set_mode = CharSetMode::Koch;
        s.curriculum.mixed_letters_percent = 33;
        s.curriculum.custom_set = vec!['Q', 'Z'];
        s.curriculum.custom_sequence = vec!['Z', 'Q'];
        s.curriculum.sequence_is_custom = true;
        s.curriculum.practice_window = Some(PracticeWindow::Last3);
        s.curriculum.sliding_window_start = 3;
        s.curriculum.sliding_window_end = 9;
        s.playback.char_wpm_min = 31.0;
        s.playback.char_wpm_max = 33.0;
        s.playback.link_char_wpm = true;
        s.playback.effective_wpm_min = 11.0;
        s.playback.effective_wpm_max = 13.0;
        s.playback.link_effective_wpm = true;
        s.playback.link_char_to_effective = false;
        s.playback.extra_word_space_multiplier = 2.5;
        s.playback.group_timeout = 9_000.0;
        s.playback.lock_input_during_group_playback = false;
        s.playback.group_repeat_min = 2;
        s.playback.group_repeat_max = 4;
        s.playback.link_group_repeat = true;
        s.band.side_tone_min = 700.0;
        s.band.side_tone_max = 900.0;
        s.band.volume_min = 0.3;
        s.band.volume_max = 0.6;
        s.band.link_volume = true;
        s.band.steepness = 7.0;
        s.band.envelope_smoothing = 0.7;
        s.band.qsb_enabled = false;
        s.band.qsb_depth = 0.9;
        s.band.qsb_rate_hz = 0.9;
        s.band.qrn_enabled = false;
        s.band.qrn_level = 0.9;
        s.band.receiver_enabled = false;
        s.band.receiver_level = 0.9;
        s.band.receiver_profile = ReceiverProfile::Whistle;
        s.band.filter_bandwidth_hz = 250.0;
        s.band.stations_min = 2;
        s.band.stations_max = 4;
        s.band.pileup_spread_hz = 400.0;
        s.band.pileup_level_db = 9.0;
        s.band.receiver_background_gain = 3.0;
        s.band.receiver_background_excitation_rate = 22.0;
        s.band.receiver_background_resonance = 30.0;
        s.band.receiver_background_decay = 0.9;
        s.band.receiver_background_offset_hz = 40.0;
        s.band.receiver_background_offset_mod_depth_hz = 12.0;
        s.band.receiver_background_offset_mod_rate_hz = 3.0;
        s.auto_level.auto_adjust_level = false;
        s.auto_level.auto_adjust_threshold = 77.0;
        s.auto_level.auto_adjust_below_threshold_count = 4;
        s.auto_level.auto_adjust_above_threshold_count = 9;
        s.auto_level.mixed_auto_level_next_axis = MixedAutoLevelAxis::Digits;
        s.auto_level.error_weight_strength = 0.9;
        s.auto_level.char_sampling_coverage_strength = 0.9;
        s.auto_level.char_sampling_thompson = false;
        s
    }
}
