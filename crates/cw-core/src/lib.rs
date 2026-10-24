//! Pure Morse group-training domain: Koch pools, Farnsworth timing, scoring, session machine.
//!
//! No React, no DOM, no audio backend. The app consumes [`PlaybackPlan`]s and
//! drives a [`SessionMachine`]. What it uses is re-exported here; the modules
//! themselves are private, so anything nobody uses shows up as dead code.

#![forbid(unsafe_code)]

mod aim;
mod alignment;
mod auto_level;
pub mod band;
mod belief;
mod board;
mod callsign;
mod contest;
mod endurance;
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
mod span;
mod speed;
mod stats;
mod streak;
mod timing;
mod words;

pub use alignment::{AlignmentPair, CopyTally, LetterAccuracy, align_group};
pub use auto_level::{
    AutoLevelChange, AutoLevelCounters, AutoLevelProgress, Completion, CountersKey, CountersUpdate,
    auto_level_progress, complete_session, evaluate_auto_level,
};
pub use belief::{Estimate, EstimatePoint};
pub use board::{
    BOARD_MIN_GROUPS, BOARD_RUN_MIN_S, BoardEntry, BoardOf, FORM_SESSIONS, Ranking, RunEntry,
    TimelinePoint, assisted_runs, board_kinds, board_modes, board_place, contest_runs,
    latest_run_length, per_group, rating, run_kinds, run_lengths, score_board, score_timeline,
};
pub use callsign::{
    CALLSIGN_TIER_MAX, CallSpec, CallsignParts, Countries, GeneratedCall, call_fault,
    continent_name, country_name, country_of, generate_call, generate_callsign, parse_callsign,
    tier_examples, wpx_prefix,
};
pub use contest::{
    AirView, CallSource, CallerHabits, ContestEffect, ContestEvent, ContestKind, ContestLog,
    ContestMachine, ContestPhase, ContestRules, ContestRun, ContestScore, ContestView, EntryField,
    Esm, ExchangeState, Fade, Field, Fields, Fill, HeardQsos, HeardTurn, LogRow, MONITOR_LEVEL,
    MessagePart, Multiplier, MyStation, OpKey, PauseWhy, PerSide, Points, QsoCheck, QsoError,
    QsoRecord, QsoState, QsoText, Rates, ReadField, Reading, RowStatus, RunClock, RunErrors,
    RunQso, RunStyle, RunSummary, SPRINT_SERIALS, ScoredQso, Scoring, Segment, Sender, SerialFit,
    SerialModel, ShownText, Side, StoredLog, Tally, TuCall, Turn, WPX_SERIALS, check_qso,
    contest_score, cut_power, cut_serial, cut_zone, esm, filter_capacity, hear_qsos,
    home_continent, home_country, keyed_token, loggable, message_text, normalize_call,
    parse_exchange, place_name, plan_turn, rate_at, read_number, section_name, send_power,
    send_report, send_serial, send_zone, space_to_call,
};
pub use endurance::{
    DROP_POINTS, ENDURANCE_MIN_SESSIONS, Endurance, EndurancePoint, GROUPS_A_STRETCH,
    MINUTES_A_STRETCH, StretchUnit, endurance,
};
pub use heatmap::{
    HEATMAP_MONTHS, HeatmapCell, HeatmapColorMode, HeatmapGrid, HeatmapMonth, build_heatmap,
};
pub use keyer::{Keyer, KeyerMode, KeyerOutput, Paddle, paddle_from_bracket};
pub use machine::{
    AUTO_CONFIRM_DELAY_MS, AnswerClose, GroupSource, SessionEffect, SessionEvent, SessionMachine,
};
pub use morse::{MAX_DIGITS_LEVEL, morse_for};
pub use pool::{
    compute_char_pool, current_practice_window, newest_unlocked, unlocked_practice_count,
};
pub use rng::{Rng, SplitMix64};
pub use sampling::{
    CharSamplingState, Sampler, adaptive_speed_on, adapts_group_size, adapts_speed, aim_section,
    generate_training_group,
};
pub use sequences::{PresetId, SEQUENCE_PRESETS, SequenceChoice, SequencePreset};
pub use session::{
    Answered, ChosenSpeed, Group, GroupResult, GroupSession, KeyTimes, MonoMs, RECOGNITION_MAX_MS,
    SessionId, SessionPhase, SessionResult, SessionTiming, SessionView, answer_length_matches,
};
pub use settings::{
    ADJUST_COUNT_MAX, ADJUST_THRESHOLD_MAX_PCT, ADJUST_THRESHOLD_MIN_PCT, Aim, AutoLevelSettings,
    BandSettings, CONTEST_MINUTES_MAX, COUNTDOWN_MAX_SEC, COVERAGE_STRENGTH_MAX, CharSetMode,
    ContestPreset, ContestSettings, CurriculumSettings, DEFAULT_FILTER_WIDTH_HZ, ERROR_WEIGHT_MAX,
    EXTRA_WORD_SPACE_MAX, EXTRA_WORD_SPACE_MIN, Experience, FILTER_WIDTH_MAX, FILTER_WIDTH_MIN,
    FilterShape, GROUP_PAUSE_MAX_SEC, GROUP_REPEAT_MAX, GROUP_REPEAT_MIN, GROUP_SIZE_MAX,
    GROUP_SIZE_MIN, GROUP_TIMEOUT_MAX_SEC, LevelAxis, MY_CALL_MAX, MY_NAME_MAX, MY_POWER_MAX,
    MY_SENDING_MAX_DB, MY_SENDING_OFF_DB, MY_ZONE_MAX, MixedAutoLevelAxis, NUM_GROUPS_MAX,
    NUM_GROUPS_MIN, PILEUP_LEVEL_MAX_DB, PILEUP_LEVEL_MIN_DB, PILEUP_SPREAD_MAX, PILEUP_SPREAD_MIN,
    PlaybackSettings, PracticeWindow, QSB_RATE_MAX_HZ, QSB_RATE_MIN_HZ, RECOGNITION_BAND_MAX_MS,
    RECOGNITION_BAND_MIN_MS, RangeSetting, RangeValues, RecognitionEdge, SIDE_TONE_MAX_HZ,
    SIDE_TONE_MIN_HZ, STATIONS_MAX, STATIONS_MIN, STEEPNESS_MAX_MS, STEEPNESS_MIN_MS,
    SettingsSection, TrainingSettings, VOLUME_MAX, VOLUME_MIN, WPM_MAX, WPM_MIN, class_reading,
    power_reading, section_code, state_code,
};
pub use span::LengthForecast;
pub use speed::{SpeedForecast, SpeedPath};
pub use stats::{
    AccuracyPoint, BUILDING_MIN_ACCURACY, BigramCell, BigramHeatmap, CONTEST_TIMES_NOTE, CharTally,
    CharacterDiagnostic, ConfusionEntry, GROUP_START_BIGRAM_TOKEN, HARD_PAIR_MIN_TRIES,
    HARD_PAIR_RATIO, HardPair, MASTERED_MIN_ACCURACY, MASTERED_MIN_ATTEMPTS, MasteryStatus,
    MistakeStats, RECENT_TRIES, RECOGNITION_MIN_TRIES, RecognitionBand, SamplingRow,
    SessionHistoryRow, accuracy_chart, character_diagnostics, group_length_forecast, mistake_stats,
    practised_modes, sampling_rows, session_history, span_history, speed_forecast, speed_history,
};
pub use streak::{StreakState, StreakStatus, compute_streak_status};
pub use timing::{
    EnvelopePoint, EnvelopeShape, Interferer, PlannedTransmission, PlaybackPlan, SEND_RELEASE_SEC,
    StationVoice, ToneEvent, Transmission, compute_after_group_gap_ms, compute_group_gap_for_wpm,
    envelope_shape, plan_morse_playback_for, plan_transmission, resolve_station,
};
pub use words::{STARTER_WORDS, WORD_LIST_MAX, WordList, clean_word};
