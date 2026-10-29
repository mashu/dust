//! How settings are written to a save, and read back from one, older ones
//! included.

use serde::{Deserialize, Serialize, Serializer};

use super::{
    AutoLevelSettings, BandSettings, CharSetMode, ContestSettings, CurriculumSettings,
    PlaybackSettings, PracticeWindow, RecognitionBands, TrainingSettings,
};

/// Settings as they are written: every section flattened into one object,
/// and Contest written as Callsigns with `contestActive` beside it, a key
/// that exists only on disk. A build that knows no contests then opens in
/// Callsigns at the same tier rather than failing the whole save, and its
/// next save drops Your station: settings, which a backup holds, not
/// training data.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SettingsOnDisk<'a> {
    #[serde(flatten)]
    curriculum: &'a CurriculumSettings,
    #[serde(flatten)]
    playback: &'a PlaybackSettings,
    #[serde(flatten)]
    band: &'a BandSettings,
    #[serde(flatten)]
    auto_level: &'a AutoLevelSettings,
    #[serde(flatten)]
    recognition: &'a RecognitionBands,
    #[serde(flatten)]
    contest: &'a ContestSettings,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    contest_active: bool,
}

impl Serialize for TrainingSettings {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        SettingsOnDisk {
            curriculum: &self.curriculum,
            playback: &self.playback,
            band: &self.band,
            auto_level: &self.auto_level,
            recognition: &self.recognition,
            contest: &self.contest,
            contest_active: self.curriculum.char_set_mode == CharSetMode::Contest,
        }
        .serialize(serializer)
    }
}

/// A save as it is read: today's settings, and the practice window and
/// character set as older saves kept them.
///
/// The legacy keys come first. A flattened section takes the keys it knows
/// as it reads them, so `practiceWindow` and `charSetMode` are read here,
/// as options, before the curriculum's own non-optional fields could see
/// them.
#[derive(Deserialize)]
struct SettingsSave {
    #[serde(flatten)]
    window: WindowSave,
    #[serde(flatten)]
    char_set: CharSetSave,
    #[serde(flatten)]
    curriculum: CurriculumSettings,
    #[serde(flatten)]
    playback: PlaybackSettings,
    #[serde(flatten)]
    band: BandSettings,
    #[serde(flatten)]
    auto_level: AutoLevelSettings,
    #[serde(flatten)]
    recognition: RecognitionBands,
    #[serde(flatten)]
    contest: ContestSettings,
}

/// Before the practice window was named, a save kept the first and last
/// unlocked character it drew from, counting from one.
#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct WindowSave {
    practice_window: Option<PracticeWindow>,
    sliding_window_start: Option<u32>,
    sliding_window_end: Option<u32>,
}

/// Before an unlock order could be the learner's own, a list of their own
/// was a character set of its own, Custom, unlocked from the left like any
/// order. Its list is that order now, under Koch. Contest is written as
/// Callsigns with `contestActive`.
#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct CharSetSave {
    char_set_mode: Option<ModeSave>,
    custom_set: Vec<char>,
    contest_active: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
enum ModeSave {
    Custom,
    #[serde(untagged)]
    Today(CharSetMode),
}

impl From<SettingsSave> for TrainingSettings {
    fn from(save: SettingsSave) -> Self {
        let mut settings = Self {
            curriculum: save.curriculum,
            playback: save.playback,
            band: save.band,
            auto_level: save.auto_level,
            recognition: save.recognition,
            contest: save.contest,
        };
        match save.char_set.char_set_mode {
            Some(ModeSave::Today(CharSetMode::Callsign)) if save.char_set.contest_active => {
                settings.curriculum.char_set_mode = CharSetMode::Contest;
            }
            // Nothing writes `contest` as the mode, but a save edited by hand
            // that names it means it.
            Some(ModeSave::Today(mode)) => settings.curriculum.char_set_mode = mode,
            Some(ModeSave::Custom) => {
                settings.curriculum.char_set_mode = CharSetMode::Koch;
                // An empty list unlocked the order already there, so that
                // order stays.
                if !Self::unique_alphabet(&save.char_set.custom_set).is_empty() {
                    let list: String = save.char_set.custom_set.iter().collect();
                    settings.type_sequence(&list);
                }
            }
            None => {}
        }
        let window = &save.window;
        settings.curriculum.practice_window = match (
            window.practice_window,
            window.sliding_window_start,
            window.sliding_window_end,
        ) {
            (Some(named), _, _) => named,
            (None, Some(start), Some(end)) => {
                window_from_bounds(crate::pool::unlocked_practice_count(&settings), start, end)
            }
            (None, _, _) => PracticeWindow::All,
        };
        settings
    }
}

/// The named window an old pair of bounds meant, over `unlocked`
/// characters. Bounds that match none of them were a window this version
/// cannot draw, and come back as everything.
fn window_from_bounds(unlocked: usize, start: u32, end: u32) -> PracticeWindow {
    let n = u32::try_from(unlocked).unwrap_or(u32::MAX).max(1);
    let start = start.clamp(1, n);
    let end = end.clamp(1, n);
    let (low, high) = (start.min(end), start.max(end));
    if low == 1 && high == n {
        PracticeWindow::All
    } else if n >= 3 && low == n - 2 && high == n {
        PracticeWindow::Last3
    } else if n >= 5 && low == n - 4 && high == n {
        PracticeWindow::Last5
    } else {
        PracticeWindow::All
    }
}

impl<'de> Deserialize<'de> for TrainingSettings {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        SettingsSave::deserialize(deserializer).map(Self::from)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sequences::{PresetId, SequenceChoice};
    use crate::settings::Aim;

    fn load(json: serde_json::Value) -> TrainingSettings {
        serde_json::from_value(json).expect("a save should load")
    }

    /// Level 5 unlocks six Koch characters, and old saves said which of them
    /// they drew from as a pair of bounds.
    #[test]
    fn an_old_save_without_a_named_window_is_read_from_its_bounds() {
        let save = |start: u32, end: u32| {
            load(serde_json::json!({
                "charSetMode": "koch", "level": 5,
                "slidingWindowStart": start, "slidingWindowEnd": end,
            }))
            .curriculum
            .practice_window
        };
        assert_eq!(save(4, 6), PracticeWindow::Last3);
        assert_eq!(save(2, 6), PracticeWindow::Last5);
        assert_eq!(save(1, 6), PracticeWindow::All);
        // Reversed bounds are read the right way round.
        assert_eq!(save(6, 4), PracticeWindow::Last3);
        // Bounds that match no named window fall back to everything.
        assert_eq!(save(3, 5), PracticeWindow::All);
    }

    /// A save from before adaptive group size has it off, and keeps the
    /// range it had; one written since reads back what it said.
    #[test]
    fn adaptive_group_size_is_off_in_a_save_from_before_it() {
        let old = load(serde_json::json!({
            "charSetMode": "koch", "minGroupSize": 3, "maxGroupSize": 7,
        }));
        assert!(!old.curriculum.adaptive_group_size);
        assert_eq!(old.curriculum.aim, Aim::Balanced);
        assert_eq!(
            (old.curriculum.min_group_size, old.curriculum.max_group_size),
            (3, 7)
        );

        let mut new = TrainingSettings::default();
        new.curriculum.adaptive_group_size = true;
        new.curriculum.aim = Aim::Comfortable;
        let json = serde_json::to_value(&new).expect("serialize");
        assert_eq!(json["adaptiveGroupSize"], true);
        assert_eq!(json["groupSizeTarget"], "comfortable");
        assert_eq!(load(json), new);
    }

    /// A save from before adaptive speed has it off; one written since reads
    /// back what it said.
    #[test]
    fn adaptive_speed_is_off_in_a_save_from_before_it() {
        let old = load(serde_json::json!({ "charWpmMin": 18.0, "charWpmMax": 30.0 }));
        assert!(!old.playback.adaptive_speed);
        let mut new = TrainingSettings::default();
        new.playback.adaptive_speed = true;
        let json = serde_json::to_value(&new).expect("serialize");
        assert_eq!(json["adaptiveSpeed"], true);
        assert_eq!(load(json), new);
    }

    #[test]
    fn a_named_window_wins_over_old_bounds() {
        let settings = load(serde_json::json!({
            "level": 5, "practiceWindow": "last5",
            "slidingWindowStart": 4, "slidingWindowEnd": 6,
        }));
        assert_eq!(settings.curriculum.practice_window, PracticeWindow::Last5);
        // A window saved as nothing at all, before it was ever set, is
        // everything.
        let settings = load(serde_json::json!({ "practiceWindow": null }));
        assert_eq!(settings.curriculum.practice_window, PracticeWindow::All);
    }

    #[test]
    fn the_old_bounds_are_not_written_back() {
        let json = serde_json::to_value(load(serde_json::json!({
            "slidingWindowStart": 4, "slidingWindowEnd": 6,
        })))
        .expect("settings serialize");
        assert!(json.get("slidingWindowStart").is_none());
        assert!(json.get("slidingWindowEnd").is_none());
        assert_eq!(json["practiceWindow"], "all");
    }

    /// Every default lives in a `Default` impl, and an empty save is exactly
    /// those.
    #[test]
    fn an_empty_save_is_the_defaults() {
        assert_eq!(load(serde_json::json!({})), TrainingSettings::default());
    }

    /// A save on the old Custom character set comes back on Koch, its list
    /// the unlock order: the same characters, unlocked the same way, so the
    /// level and what was learned of each character carry over.
    #[test]
    fn a_save_on_custom_comes_back_as_its_own_unlock_order() {
        let custom = load(serde_json::json!({
            "charSetMode": "custom", "level": 4,
            "customSet": ["q", "R", "S", "T", "#", "R"],
            "customSequence": ["U", "V"], "practiceWindow": "last3",
        }));
        assert_eq!(custom.curriculum.char_set_mode, CharSetMode::Koch);
        assert_eq!(custom.curriculum.custom_sequence, ['Q', 'R', 'S', 'T']);
        assert_eq!(custom.sequence_choice(), SequenceChoice::Custom);
        assert_eq!(custom.curriculum.level, 4);
        assert_eq!(custom.curriculum.practice_window, PracticeWindow::Last3);
        assert_eq!(custom.alphabet_fingerprint(), "QRST");
        let json = serde_json::to_value(&custom).expect("settings serialize");
        assert_eq!(json["charSetMode"], "koch");
        assert!(json.get("customSet").is_none(), "not written back");

        // A list that is a preset is that preset.
        let mania: Vec<char> = crate::sequences::TRADITIONAL_KOCH_SEQUENCE.to_vec();
        let preset = load(serde_json::json!({ "charSetMode": "custom", "customSet": mania }));
        assert_eq!(
            preset.sequence_choice(),
            SequenceChoice::Preset(PresetId::MorseMania)
        );

        // An empty list unlocked the order already chosen, and still does.
        let empty = load(serde_json::json!({
            "charSetMode": "custom", "customSet": [], "customSequence": ["U", "V"],
        }));
        assert_eq!(empty.curriculum.char_set_mode, CharSetMode::Koch);
        assert_eq!(empty.curriculum.custom_sequence, ['U', 'V']);

        // A list left over from a visit to Custom is no order on Koch.
        let koch = load(serde_json::json!({
            "charSetMode": "koch", "customSet": ["Q", "R"],
        }));
        assert_eq!(
            koch.sequence_choice(),
            SequenceChoice::Preset(PresetId::Lcwo)
        );
        // Every other character set reads as it was saved.
        for mode in ["koch", "digits", "mixed", "callsign", "words"] {
            let json = serde_json::json!({ "charSetMode": mode });
            assert_eq!(
                serde_json::to_value(load(json.clone())).unwrap()["charSetMode"],
                json["charSetMode"]
            );
        }
    }

    /// Contest goes to disk as Callsigns with `contestActive`, which only a
    /// build that knows contests reads, and comes back as Contest with
    /// everything it holds.
    #[test]
    fn contest_is_written_as_callsigns_and_read_back_as_contest() {
        let mut contest = TrainingSettings::default();
        contest.curriculum.char_set_mode = CharSetMode::Contest;
        contest.curriculum.callsign_level = 4;
        contest.contest.contest_minutes = 25;
        contest.contest.my_call = "DL1ABC".to_string();
        let json = serde_json::to_value(&contest).expect("settings serialize");
        assert_eq!(json["charSetMode"], "callsign");
        assert_eq!(json["contestActive"], true);
        assert_eq!(json["contestKind"], "wpx");
        assert_eq!(json["contestMinutes"], 25);
        assert_eq!(json["myCall"], "DL1ABC");
        assert_eq!(load(json), contest);

        // Every other mode is written as it is, with no mark.
        for mode in CharSetMode::ALL {
            if mode == CharSetMode::Contest {
                continue;
            }
            let mut settings = TrainingSettings::default();
            settings.curriculum.char_set_mode = mode;
            let json = serde_json::to_value(&settings).expect("settings serialize");
            assert!(json.get("contestActive").is_none(), "{mode:?}");
            assert_eq!(load(json).curriculum.char_set_mode, mode);
        }
        // The mark means Contest only beside Callsigns.
        let koch = load(serde_json::json!({ "charSetMode": "koch", "contestActive": true }));
        assert_eq!(koch.curriculum.char_set_mode, CharSetMode::Koch);
    }

    /// A pile-up is saved by its name and read back, and a run style this
    /// build does not know reads as a single caller rather than costing the
    /// rest of the save.
    #[test]
    fn a_run_style_reads_back_and_an_unknown_one_is_a_single_caller() {
        let mut pileup = TrainingSettings::default();
        pileup.contest.contest_style = crate::contest::RunStyle::Pileup;
        let json = serde_json::to_value(&pileup).expect("settings serialize");
        assert_eq!(json["contestStyle"], "pileup");
        assert_eq!(load(json), pileup);
        let unknown = load(serde_json::json!({ "contestStyle": "swarm", "contestMinutes": 25 }));
        assert_eq!(
            unknown.contest.contest_style,
            crate::contest::RunStyle::Single
        );
        assert_eq!(unknown.contest.contest_minutes, 25, "the rest of the save");
    }

    /// The callers' habits are saved by their name and read back, and
    /// habits this build does not know, or none at all, read as easy rather
    /// than costing the rest of the save.
    #[test]
    fn callers_habits_read_back_and_unknown_ones_are_easy() {
        use crate::contest::CallerHabits;
        let mut rough = TrainingSettings::default();
        rough.contest.contest_habits = CallerHabits::Rough;
        let json = serde_json::to_value(&rough).expect("settings serialize");
        assert_eq!(json["contestHabits"], "rough");
        assert_eq!(load(json), rough);
        let unknown = load(serde_json::json!({ "contestHabits": "lids", "contestMinutes": 25 }));
        assert_eq!(unknown.contest.contest_habits, CallerHabits::Easy);
        assert_eq!(unknown.contest.contest_minutes, 25, "the rest of the save");
        let older = load(serde_json::json!({ "contestMinutes": 25 }));
        assert_eq!(older.contest.contest_habits, CallerHabits::Easy);
    }

    /// Show copy is saved as a switch and read back; anything else in its
    /// place, or nothing, reads as off rather than costing the rest of the
    /// save.
    #[test]
    fn show_copy_reads_back_and_anything_else_is_off() {
        let mut shown = TrainingSettings::default();
        shown.contest.contest_show_copy = true;
        let json = serde_json::to_value(&shown).expect("settings serialize");
        assert_eq!(json["contestShowCopy"], true);
        assert_eq!(load(json), shown);
        for odd in [
            serde_json::json!("yes"),
            serde_json::json!(1),
            serde_json::Value::Null,
        ] {
            let read = load(serde_json::json!({ "contestShowCopy": odd, "contestMinutes": 25 }));
            assert!(!read.contest.contest_show_copy, "{odd}");
            assert_eq!(read.contest.contest_minutes, 25, "the rest of the save");
        }
        let older = load(serde_json::json!({ "contestMinutes": 25 }));
        assert!(!older.contest.contest_show_copy);
    }

    /// Flutter and full break-in are saved as switches and read back; both
    /// ship off, so a save from before them, or anything but true or false
    /// in their place, reads as off, costing the rest of the save nothing.
    #[test]
    fn flutter_and_full_break_in_read_back_and_anything_else_is_off() {
        let mut on = TrainingSettings::default();
        on.contest.contest_flutter = true;
        on.contest.contest_qsk = true;
        let json = serde_json::to_value(&on).expect("settings serialize");
        assert_eq!(
            (&json["contestFlutter"], &json["contestQsk"]),
            (&true.into(), &true.into())
        );
        assert_eq!(load(json), on);
        for odd in [
            serde_json::json!("yes"),
            serde_json::json!(1),
            serde_json::Value::Null,
        ] {
            let read = load(serde_json::json!({
                "contestFlutter": odd,
                "contestQsk": odd,
                "contestMinutes": 25,
            }));
            assert!(!read.contest.contest_flutter, "{odd}");
            assert!(!read.contest.contest_qsk, "{odd}");
            assert_eq!(read.contest.contest_minutes, 25, "the rest of the save");
        }
        let older = load(serde_json::json!({ "contestMinutes": 25 }));
        assert!(!older.contest.contest_flutter && !older.contest.contest_qsk);
    }

    /// The callers' adaptive speed is saved as a switch and read back; it
    /// ships on, so a save from before it, or anything but true or false in
    /// its place, reads as on, costing the rest of the save nothing.
    #[test]
    fn the_callers_adaptive_speed_reads_back_and_anything_else_is_on() {
        let mut fixed = TrainingSettings::default();
        fixed.contest.contest_adapt_speed = false;
        let json = serde_json::to_value(&fixed).expect("settings serialize");
        assert_eq!(json["contestAdaptSpeed"], false);
        assert_eq!(load(json), fixed);
        for odd in [
            serde_json::json!("no"),
            serde_json::json!(0),
            serde_json::Value::Null,
        ] {
            let read = load(serde_json::json!({ "contestAdaptSpeed": odd, "contestMinutes": 25 }));
            assert!(read.contest.contest_adapt_speed, "{odd}");
            assert_eq!(read.contest.contest_minutes, 25, "the rest of the save");
        }
        let older = load(serde_json::json!({ "contestMinutes": 25 }));
        assert!(older.contest.contest_adapt_speed);
        assert!(
            !older.playback.adaptive_speed,
            "group training's is its own"
        );
    }

    /// A build that knows no contests reads Contest as Callsigns at the same
    /// tier: the mode is one it knows, and `contestActive` a key it skips.
    #[test]
    fn a_build_before_contests_opens_contest_in_callsigns() {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct BeforeContests {
            char_set_mode: ModeBeforeContests,
            callsign_level: u32,
        }
        #[derive(Debug, PartialEq, Deserialize)]
        #[serde(rename_all = "lowercase")]
        enum ModeBeforeContests {
            Koch,
            Digits,
            Mixed,
            Callsign,
            Words,
        }
        let mut contest = TrainingSettings::default();
        contest.curriculum.char_set_mode = CharSetMode::Contest;
        contest.curriculum.callsign_level = 5;
        let json = serde_json::to_value(&contest).expect("settings serialize");
        let older: BeforeContests = serde_json::from_value(json).expect("an older build reads it");
        assert_eq!(older.char_set_mode, ModeBeforeContests::Callsign);
        assert_eq!(older.callsign_level, 5);
        let named = serde_json::json!({ "charSetMode": "contest", "callsignLevel": 5 });
        assert!(serde_json::from_value::<BeforeContests>(named).is_err());
    }

    /// Nothing writes `contest` as the mode; a save edited by hand that
    /// names it opens in Contest, and is written back the way Contest is.
    #[test]
    fn a_save_that_names_contest_opens_in_contest() {
        let settings = load(serde_json::json!({ "charSetMode": "contest", "callsignLevel": 4 }));
        assert_eq!(settings.curriculum.char_set_mode, CharSetMode::Contest);
        assert_eq!(settings.curriculum.callsign_level, 4);
        let json = serde_json::to_value(&settings).expect("settings serialize");
        assert_eq!(json["charSetMode"], "callsign");
        assert_eq!(json["contestActive"], true);
    }

    /// A contest this build does not know is the default one, and costs
    /// nothing else in the save.
    #[test]
    fn an_unknown_contest_reads_as_wpx() {
        let settings = load(serde_json::json!({ "contestKind": "cwt", "contestMinutes": 30 }));
        assert_eq!(
            settings.contest.contest_kind,
            crate::contest::ContestKind::Wpx
        );
        assert_eq!(settings.contest.contest_minutes, 30);
    }

    /// The first versions named two settings after the Koch method. Their
    /// saves still load.
    #[test]
    fn the_first_versions_names_still_load() {
        let settings = load(serde_json::json!({
            "charSetMode": "koch", "kochLevel": 5, "autoAdjustKoch": false,
        }));
        assert_eq!(settings.curriculum.char_set_mode, CharSetMode::Koch);
        assert_eq!(settings.curriculum.level, 5);
        assert!(!settings.auto_level.auto_adjust_level);
    }

    /// Character sampling had a Thompson switch, which did nothing on
    /// average and was taken out. A save with it switched on still loads,
    /// the rest of it as it was, and the next save no longer writes it.
    #[test]
    fn a_save_from_before_the_thompson_switch_went_still_loads() {
        let settings = load(serde_json::json!({
            "charSamplingThompson": true,
            "errorWeightStrength": 4.5,
            "charSamplingCoverageStrength": 0.5,
            "level": 7,
        }));
        assert_eq!(settings.auto_level.error_weight_strength, 4.5);
        assert_eq!(settings.auto_level.char_sampling_coverage_strength, 0.5);
        assert_eq!(settings.curriculum.level, 7);
        let saved = serde_json::to_value(&settings).expect("settings write");
        assert!(saved.get("charSamplingThompson").is_none(), "{saved}");
        assert!(saved.get("errorWeightStrength").is_some());
    }
}
