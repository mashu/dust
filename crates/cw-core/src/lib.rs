//! Pure Morse group-training domain: Koch pools, Farnsworth timing, scoring, session machine.
//!
//! No React, no DOM, no audio backend. The WASM/UI crate consumes [`timing::PlaybackPlan`]
//! and drives [`session::GroupSession`].

pub mod alignment;
pub mod auto_level;
pub mod band;
pub mod heatmap;
pub mod level;
pub mod morse;
pub mod pool;
pub mod rng;
pub mod sampling;
pub mod score;
pub mod sequences;
pub mod session;
pub mod settings;
pub mod stats;
pub mod streak;
pub mod timing;

pub use alignment::{
    align_group, calculate_group_letter_accuracy, calculate_overall_character_accuracy,
    AlignmentPair, LetterAccuracy,
};
pub use auto_level::{
    apply_auto_level, auto_level_progress, evaluate_auto_level, AutoAdjustMode, AutoLevelCounters,
    AutoLevelProgress, AutoLevelResult,
};
pub use heatmap::{
    build_heatmap, HeatmapCell, HeatmapColorMode, HeatmapGrid, HEATMAP_WEEKS,
};
pub use level::{
    max_level_for_len, unlocked_count_for_level, unlocked_prefix, LEVEL_MIN,
};
pub use morse::{
    decode_morse_pattern, digits_unlocked_count, is_morse_code_prefix, morse_for,
    MixedAutoLevelAxis, DEFAULT_SLIDING_WINDOW_END, DEFAULT_SLIDING_WINDOW_START, KOCH_LEVEL_MAX,
    KOCH_LEVEL_MIN, LCWO_SEQUENCE, MAX_DIGITS_LEVEL,
};
pub use sequences::{
    apply_custom_sequence, apply_sequence_preset, preset_by_id, preset_id_for, sequence_preset_id,
    SequencePreset, SEQUENCE_PRESETS,
};
pub use pool::{
    apply_practice_window, compute_char_pool, current_practice_window, fit_settings_to_alphabet,
    unlocked_practice_count,
};
pub use rng::{weighted_random_pick, FastrandRng, Rng};
pub use sampling::{
    create_initial_sampling_state, generate_training_group, update_sampling_state_from_answer,
    CharSamplingState,
};
pub use session::{
    answer_length_matches, build_session_result, GroupResult, GroupSession, RuntimeStatus,
    SessionResult, SessionSummary, SessionTiming,
};
pub use settings::{CharSetMode, PracticeWindow, QrmProfile, TrainingSettings};
pub use stats::{
    accuracy_chart, bigram_heatmap, character_diagnostics, confusion_entries, sampling_rows,
    session_history, unigram_stats, AccuracyPoint, BigramHeatmap, CharacterDiagnostic,
    ConfusionEntry, MasteryStatus, SamplingRow, SessionHistoryRow, UnigramStat,
    GROUP_START_BIGRAM_TOKEN,
};
pub use streak::{compute_streak_status, StreakState, StreakStatus};
pub use timing::{compute_group_gap_ms, plan_morse_playback, PlaybackPlan, ToneEvent};
