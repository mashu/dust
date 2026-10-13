//! Pure Morse group-training domain: Koch pools, Farnsworth timing, scoring, session machine.
//!
//! No React, no DOM, no audio backend. The app consumes [`PlaybackPlan`]s and
//! drives a [`SessionMachine`]. What it uses is re-exported here; the modules
//! themselves are private, so anything nobody uses shows up as dead code.

#![forbid(unsafe_code)]

mod alignment;
mod auto_level;
pub mod band;
mod callsign;
mod heatmap;
mod keyer;
mod level;
mod machine;
mod morse;
mod pool;
mod rng;
mod sampling;
mod score;
mod sequences;
mod session;
mod settings;
#[cfg(test)]
mod soak;
mod stats;
mod streak;
mod timing;

pub use alignment::{AlignmentPair, CopyTally, LetterAccuracy, align_group};
pub use auto_level::{
    AutoLevelChange, AutoLevelCounters, AutoLevelProgress, Completion, CountersKey, CountersUpdate,
    auto_level_progress, complete_session, evaluate_auto_level,
};
pub use callsign::{CALLSIGN_TIER_MAX, generate_callsign, tier_examples};
pub use heatmap::{HEATMAP_WEEKS, HeatmapCell, HeatmapColorMode, HeatmapGrid, build_heatmap};
pub use keyer::{Keyer, KeyerMode, KeyerOutput, Paddle, paddle_from_bracket};
pub use machine::{
    AUTO_CONFIRM_DELAY_MS, AnswerClose, GroupSource, SessionEffect, SessionEvent, SessionMachine,
};
pub use morse::{MAX_DIGITS_LEVEL, morse_for};
pub use pool::{
    compute_char_pool, current_practice_window, newest_unlocked, unlocked_practice_count,
};
pub use rng::{Rng, SplitMix64};
pub use sampling::{CharSamplingState, Sampler, generate_training_group};
pub use sequences::{PresetId, SEQUENCE_PRESETS, SequenceChoice, SequencePreset};
pub use session::{
    Group, GroupResult, GroupSession, MonoMs, SessionId, SessionPhase, SessionResult,
    SessionTiming, SessionView, answer_length_matches,
};
pub use settings::{
    ADJUST_COUNT_MAX, ADJUST_THRESHOLD_MAX_PCT, ADJUST_THRESHOLD_MIN_PCT, AutoLevelSettings,
    BandSettings, COVERAGE_STRENGTH_MAX, CharSetMode, CurriculumSettings, DEFAULT_FILTER_WIDTH_HZ,
    ERROR_WEIGHT_MAX, EXTRA_WORD_SPACE_MAX, EXTRA_WORD_SPACE_MIN, FILTER_WIDTH_MAX,
    FILTER_WIDTH_MIN, FilterShape, GROUP_PAUSE_MAX_SEC, GROUP_REPEAT_MAX, GROUP_REPEAT_MIN,
    GROUP_SIZE_MAX, GROUP_SIZE_MIN, GROUP_TIMEOUT_MAX_SEC, LevelAxis, MixedAutoLevelAxis,
    NUM_GROUPS_MAX, NUM_GROUPS_MIN, PILEUP_LEVEL_MAX_DB, PILEUP_LEVEL_MIN_DB, PILEUP_SPREAD_MAX,
    PILEUP_SPREAD_MIN, PlaybackSettings, PracticeWindow, QSB_RATE_MAX_HZ, QSB_RATE_MIN_HZ,
    RangeSetting, RangeValues, SIDE_TONE_MAX_HZ, SIDE_TONE_MIN_HZ, STATIONS_MAX, STATIONS_MIN,
    STEEPNESS_MAX_MS, STEEPNESS_MIN_MS, SettingsSection, TrainingSettings, VOLUME_MAX, VOLUME_MIN,
    WPM_MAX, WPM_MIN,
};
pub use stats::{
    AccuracyPoint, BigramHeatmap, CharacterDiagnostic, ConfusionEntry, GROUP_START_BIGRAM_TOKEN,
    MasteryStatus, MistakeStats, SamplingRow, SessionHistoryRow, accuracy_chart,
    character_diagnostics, mistake_stats, sampling_rows, session_history,
};
pub use streak::{StreakState, StreakStatus, compute_streak_status};
pub use timing::{
    EnvelopePoint, EnvelopeShape, Interferer, PlannedTransmission, PlaybackPlan, StationVoice,
    ToneEvent, Transmission, compute_after_group_gap_ms, compute_group_gap_for_wpm, envelope_shape,
    plan_morse_playback_for, plan_transmission, resolve_station,
};
