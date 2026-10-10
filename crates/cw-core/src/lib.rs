//! Pure Morse group-training domain: Koch pools, Farnsworth timing, scoring, session machine.
//!
//! No React, no DOM, no audio backend. The WASM/UI crate consumes [`timing::PlaybackPlan`]
//! and drives [`session::GroupSession`].

#![forbid(unsafe_code)]

pub mod alignment;
pub mod auto_level;
pub mod band;
pub mod callsign;
pub mod heatmap;
pub mod keyer;
pub mod level;
pub mod machine;
pub mod morse;
pub mod pool;
pub mod rng;
pub mod sampling;
pub mod score;
pub mod sequences;
pub mod session;
pub mod settings;
#[cfg(test)]
mod soak;
pub mod stats;
pub mod streak;
pub mod timing;

pub use alignment::{
    AlignmentPair, LetterAccuracy, align_group, calculate_group_letter_accuracy,
    calculate_overall_character_accuracy,
};
pub use auto_level::{
    AutoAdjustMode, AutoLevelCounters, AutoLevelProgress, AutoLevelResult, apply_auto_level,
    auto_level_progress, evaluate_auto_level,
};
pub use callsign::{
    CALLSIGN_TIER_MAX, CALLSIGN_TIER_MIN, CallsignParts, callsign_pool, generate_callsign,
    parse_callsign, tier_examples,
};
pub use heatmap::{HEATMAP_WEEKS, HeatmapCell, HeatmapColorMode, HeatmapGrid, build_heatmap};
pub use keyer::{
    KeyerMode, LETTER_GAP_DITS, Paddle, PaddleDecoder, PaddleKeyer, STRAIGHT_DAH_DITS,
    dit_ms_for_wpm, paddle_from_bracket,
};
pub use level::{LEVEL_MIN, max_level_for_len, unlocked_count_for_level, unlocked_prefix};
pub use machine::{
    AUTO_CONFIRM_DELAY_MS, SessionEffect, SessionEvent, SessionMachine, SessionPhase,
};
pub use morse::{
    DEFAULT_SLIDING_WINDOW_END, DEFAULT_SLIDING_WINDOW_START, KOCH_LEVEL_MAX, KOCH_LEVEL_MIN,
    LCWO_SEQUENCE, MAX_DIGITS_LEVEL, decode_morse_pattern, digits_unlocked_count,
    is_morse_code_prefix, morse_for,
};
pub use pool::{
    apply_practice_window, compute_char_pool, current_practice_window, fit_settings_to_alphabet,
    unlocked_practice_count,
};
pub use rng::{FastrandRng, Rng, weighted_random_pick};
pub use sampling::{
    CharSamplingState, create_initial_sampling_state, generate_callsign_group,
    generate_training_group, update_sampling_state_from_answer,
};
pub use sequences::{
    SEQUENCE_PRESETS, SequencePreset, apply_custom_sequence, apply_sequence_preset, preset_by_id,
    preset_id_for, sequence_preset_id,
};
pub use session::{
    Group, GroupResult, GroupSession, RuntimeStatus, SessionId, SessionResult, SessionSummary,
    SessionTiming, SessionView, answer_length_matches, build_session_result,
};
pub use settings::{
    AutoLevelSettings, BandSettings, CharSetMode, CurriculumSettings, FILTER_BANDWIDTH_MAX,
    FILTER_BANDWIDTH_MIN, FilterShape, GROUP_REPEAT_MAX, GROUP_REPEAT_MIN, MixedAutoLevelAxis,
    PILEUP_LEVEL_MAX_DB, PILEUP_LEVEL_MIN_DB, PILEUP_SPREAD_MAX, PILEUP_SPREAD_MIN,
    PlaybackSettings, PracticeWindow, RangeSetting, RangeValues, STATIONS_MAX, STATIONS_MIN,
    SettingsSection, TrainingSettings,
};
pub use stats::{
    AccuracyPoint, BigramHeatmap, CharacterDiagnostic, ConfusionEntry, GROUP_START_BIGRAM_TOKEN,
    MasteryStatus, SamplingRow, SessionHistoryRow, UnigramStat, accuracy_chart, bigram_heatmap,
    character_diagnostics, confusion_entries, sampling_rows, session_history, unigram_stats,
};
pub use streak::{StreakState, StreakStatus, compute_streak_status};
pub use timing::{
    EnvelopePoint, EnvelopeShape, Interferer, PlannedTransmission, PlaybackPlan, StationVoice,
    ToneEvent, Transmission, build_envelope_curve, compute_after_group_gap_ms,
    compute_group_gap_for_wpm, compute_group_gap_ms, dot_seconds, envelope_shape,
    plan_morse_playback, plan_morse_playback_for, plan_transmission, resolve_group_repeats,
    resolve_pileup, resolve_station,
};
