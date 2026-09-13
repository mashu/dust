//! Koch character-sequence presets.

use crate::morse::LCWO_SEQUENCE;

pub const TRADITIONAL_KOCH_SEQUENCE: &[char] = &[
    'K', 'M', 'R', 'S', 'U', 'A', 'P', 'T', 'L', 'O', 'W', 'I', 'N', 'J', 'E', 'F', '0', 'Y', 'V',
    'G', '5', 'Q', '9', 'Z', 'H', '3', '8', 'B', '?', '4', '2', '7', 'C', '1', '6', 'D', 'X', '/',
    '=', '+',
];

pub const ALPHABETICAL_SEQUENCE: &[char] = &[
    'A', 'B', 'C', 'D', 'E', 'F', 'G', 'H', 'I', 'J', 'K', 'L', 'M', 'N', 'O', 'P', 'Q', 'R', 'S',
    'T', 'U', 'V', 'W', 'X', 'Y', 'Z', '0', '1', '2', '3', '4', '5', '6', '7', '8', '9', '/', '=',
    '+', '?',
];

pub const CW_ACADEMY_SEQUENCE: &[char] = &[
    'E', 'T', 'A', 'O', 'N', 'I', 'R', 'S', 'H', 'D', 'L', 'U', 'C', 'M', 'W', 'F', 'Y', 'P', 'G',
    'B', 'V', 'K', 'J', 'X', 'Q', 'Z', '0', '1', '2', '3', '4', '5', '6', '7', '8', '9', '/', '=',
    '+', '?', '.', ',',
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SequencePreset {
    pub id: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    pub sequence: &'static [char],
}

pub const SEQUENCE_PRESETS: &[SequencePreset] = &[
    SequencePreset {
        id: "lcwo",
        name: "LCWO",
        description: "Learn CW Online Koch sequence",
        sequence: LCWO_SEQUENCE,
    },
    SequencePreset {
        id: "morsemania",
        name: "MorseMania",
        description: "Traditional Koch method sequence",
        sequence: TRADITIONAL_KOCH_SEQUENCE,
    },
    SequencePreset {
        id: "cw-academy",
        name: "CW Academy",
        description: "English-frequency beginner groups",
        sequence: CW_ACADEMY_SEQUENCE,
    },
    SequencePreset {
        id: "alphabetical",
        name: "Alphabetical",
        description: "A–Z then digits and punctuation",
        sequence: ALPHABETICAL_SEQUENCE,
    },
];

pub fn preset_id_for(sequence: &[char]) -> &'static str {
    if sequence.is_empty() {
        return "lcwo";
    }
    for preset in SEQUENCE_PRESETS {
        if preset.sequence == sequence {
            return preset.id;
        }
    }
    "custom"
}

/// UI selection for Sequence pills. Custom stays selected even if the order matches a preset.
pub fn sequence_preset_id(settings: &crate::settings::TrainingSettings) -> &'static str {
    if settings.curriculum.sequence_is_custom {
        "custom"
    } else {
        preset_id_for(settings.sequence())
    }
}

pub fn preset_by_id(id: &str) -> Option<&'static SequencePreset> {
    SEQUENCE_PRESETS.iter().find(|p| p.id == id)
}

pub fn apply_custom_sequence(settings: &mut crate::settings::TrainingSettings) {
    settings.curriculum.custom_sequence = settings.sequence().to_vec();
    settings.curriculum.sequence_is_custom = true;
}

pub fn apply_sequence_preset(settings: &mut crate::settings::TrainingSettings, id: &str) {
    if id == "custom" {
        apply_custom_sequence(settings);
        return;
    }
    settings.curriculum.sequence_is_custom = false;
    if id == "lcwo" {
        settings.curriculum.custom_sequence.clear();
        return;
    }
    if let Some(preset) = preset_by_id(id) {
        settings.curriculum.custom_sequence = preset.sequence.to_vec();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::TrainingSettings;

    #[test]
    fn empty_is_lcwo() {
        assert_eq!(preset_id_for(&[]), "lcwo");
        assert_eq!(preset_id_for(LCWO_SEQUENCE), "lcwo");
    }

    #[test]
    fn custom_stays_selected_when_order_matches_preset() {
        let mut settings = TrainingSettings::default();
        apply_custom_sequence(&mut settings);
        assert_eq!(sequence_preset_id(&settings), "custom");
        assert_eq!(settings.curriculum.custom_sequence, LCWO_SEQUENCE);
        apply_sequence_preset(&mut settings, "lcwo");
        assert_eq!(sequence_preset_id(&settings), "lcwo");
        assert!(!settings.curriculum.sequence_is_custom);
    }

    #[test]
    fn preset_click_leaves_custom_mode() {
        let mut settings = TrainingSettings::default();
        apply_sequence_preset(&mut settings, "morsemania");
        apply_custom_sequence(&mut settings);
        assert_eq!(sequence_preset_id(&settings), "custom");
        apply_sequence_preset(&mut settings, "alphabetical");
        assert_eq!(sequence_preset_id(&settings), "alphabetical");
        assert!(!settings.curriculum.sequence_is_custom);
    }
}

#[cfg(test)]
mod preset_tests {
    use super::*;
    use crate::settings::TrainingSettings;

    #[test]
    fn an_unknown_order_is_reported_as_custom() {
        assert_eq!(preset_id_for(&[]), "lcwo");
        assert_eq!(preset_id_for(LCWO_SEQUENCE), "lcwo");
        assert_eq!(preset_id_for(ALPHABETICAL_SEQUENCE), "alphabetical");
        assert_eq!(preset_id_for(&['K', 'M']), "custom");
        assert!(preset_by_id("nope").is_none());
        assert_eq!(
            preset_by_id("cw-academy").map(|p| p.name),
            Some("CW Academy")
        );
    }

    #[test]
    fn choosing_custom_keeps_the_order_that_was_showing() {
        let mut settings = TrainingSettings::default();
        apply_sequence_preset(&mut settings, "morsemania");
        assert_eq!(sequence_preset_id(&settings), "morsemania");

        apply_sequence_preset(&mut settings, "custom");
        assert!(settings.curriculum.sequence_is_custom);
        assert_eq!(
            settings.curriculum.custom_sequence,
            TRADITIONAL_KOCH_SEQUENCE
        );
        // The order still matches a preset, but the user asked for custom.
        assert_eq!(sequence_preset_id(&settings), "custom");
    }

    #[test]
    fn going_back_to_lcwo_clears_the_stored_order() {
        let mut settings = TrainingSettings::default();
        apply_sequence_preset(&mut settings, "alphabetical");
        assert!(!settings.curriculum.custom_sequence.is_empty());
        apply_sequence_preset(&mut settings, "lcwo");
        assert!(settings.curriculum.custom_sequence.is_empty());
        assert!(!settings.curriculum.sequence_is_custom);
        assert_eq!(settings.sequence(), LCWO_SEQUENCE);
    }

    #[test]
    fn an_unknown_preset_id_leaves_the_order_alone() {
        let mut settings = TrainingSettings::default();
        apply_sequence_preset(&mut settings, "alphabetical");
        apply_sequence_preset(&mut settings, "nonsense");
        assert_eq!(settings.curriculum.custom_sequence, ALPHABETICAL_SEQUENCE);
    }

    #[test]
    fn every_preset_teaches_only_sendable_characters() {
        for preset in SEQUENCE_PRESETS {
            assert!(!preset.sequence.is_empty(), "{} is empty", preset.id);
            assert!(!preset.description.is_empty());
            for ch in preset.sequence {
                assert!(
                    crate::morse::morse_for(*ch).is_some(),
                    "{} teaches {ch:?}, which has no Morse code",
                    preset.id
                );
            }
            let mut unique = preset.sequence.to_vec();
            unique.sort_unstable();
            unique.dedup();
            assert_eq!(
                unique.len(),
                preset.sequence.len(),
                "{} repeats a character",
                preset.id
            );
        }
    }
}
