//! How settings are read back from a save, older ones included.

use serde::Deserialize;

use super::{
    AutoLevelSettings, BandSettings, CharSetMode, CurriculumSettings, PlaybackSettings,
    PracticeWindow, TrainingSettings,
};

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
/// order. Its list is that order now, under Koch.
#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct CharSetSave {
    char_set_mode: Option<ModeSave>,
    custom_set: Vec<char>,
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
        };
        match save.char_set.char_set_mode {
            // Contest cannot be picked yet. A save that names it, edited by
            // hand, opens in Callsigns, whose tier it shares, rather than in
            // a mode no control shows and no group session may record.
            Some(ModeSave::Today(CharSetMode::Contest)) => {
                settings.curriculum.char_set_mode = CharSetMode::Callsign;
            }
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

    /// Until Contest can be picked, a save that names it opens in Callsigns
    /// at the same tier.
    #[test]
    fn a_save_on_contest_opens_in_callsigns() {
        let settings = load(serde_json::json!({ "charSetMode": "contest", "callsignLevel": 4 }));
        assert_eq!(settings.curriculum.char_set_mode, CharSetMode::Callsign);
        assert_eq!(settings.curriculum.callsign_level, 4);
        let json = serde_json::to_value(&settings).expect("settings serialize");
        assert_eq!(json["charSetMode"], "callsign");
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
}
