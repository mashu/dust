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

/// One of the unlock orders the trainer ships.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PresetId {
    Lcwo,
    MorseMania,
    CwAcademy,
    Alphabetical,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SequencePreset {
    pub id: PresetId,
    pub name: &'static str,
    pub description: &'static str,
    pub sequence: &'static [char],
}

pub const SEQUENCE_PRESETS: &[SequencePreset] = &[
    SequencePreset {
        id: PresetId::Lcwo,
        name: "LCWO",
        description: "Learn CW Online Koch sequence",
        sequence: LCWO_SEQUENCE,
    },
    SequencePreset {
        id: PresetId::MorseMania,
        name: "MorseMania",
        description: "Traditional Koch method sequence",
        sequence: TRADITIONAL_KOCH_SEQUENCE,
    },
    SequencePreset {
        id: PresetId::CwAcademy,
        name: "CW Academy",
        description: "English-frequency beginner groups",
        sequence: CW_ACADEMY_SEQUENCE,
    },
    SequencePreset {
        id: PresetId::Alphabetical,
        name: "Alphabetical",
        description: "A–Z then digits and punctuation",
        sequence: ALPHABETICAL_SEQUENCE,
    },
];

impl PresetId {
    pub fn preset(self) -> &'static SequencePreset {
        SEQUENCE_PRESETS
            .iter()
            .find(|preset| preset.id == self)
            .unwrap_or(&SEQUENCE_PRESETS[0])
    }
}

/// Which unlock order is chosen: one of the presets, or the user's own.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SequenceChoice {
    Preset(PresetId),
    /// The user's own order. Stays chosen even while it matches a preset,
    /// so editing it from one does not snap back to the preset's pill.
    Custom,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::TrainingSettings;

    #[test]
    fn a_typed_order_is_cleaned_and_kept_as_the_learners_own() {
        let mut settings = TrainingSettings::default();
        settings.type_sequence("km k?x 9");
        assert_eq!(
            settings.curriculum.custom_sequence,
            ['K', 'M', '?', 'X', '9']
        );
        assert_eq!(settings.sequence_choice(), SequenceChoice::Custom);
    }

    #[test]
    fn a_typed_order_that_is_a_preset_selects_it() {
        let mut settings = TrainingSettings::default();
        settings.choose_sequence(SequenceChoice::Custom);
        let alphabetical: String = ALPHABETICAL_SEQUENCE.iter().collect();
        settings.type_sequence(&alphabetical);
        assert_eq!(
            settings.sequence_choice(),
            SequenceChoice::Preset(PresetId::Alphabetical)
        );
        let lcwo: String = LCWO_SEQUENCE.iter().collect();
        settings.type_sequence(&lcwo);
        assert_eq!(
            settings.sequence_choice(),
            SequenceChoice::Preset(PresetId::Lcwo)
        );
        assert!(settings.curriculum.custom_sequence.is_empty());
    }

    #[test]
    fn an_empty_typed_order_is_lcwos() {
        let mut settings = TrainingSettings::default();
        settings.type_sequence("KM");
        settings.type_sequence(" ");
        assert_eq!(
            settings.sequence_choice(),
            SequenceChoice::Preset(PresetId::Lcwo)
        );
        assert_eq!(settings.sequence(), LCWO_SEQUENCE);
    }

    #[test]
    fn an_order_matching_a_preset_is_that_preset() {
        let mut settings = TrainingSettings::default();
        assert_eq!(
            settings.sequence_choice(),
            SequenceChoice::Preset(PresetId::Lcwo)
        );
        settings.curriculum.custom_sequence = LCWO_SEQUENCE.to_vec();
        assert_eq!(
            settings.sequence_choice(),
            SequenceChoice::Preset(PresetId::Lcwo)
        );
        settings.curriculum.custom_sequence = ALPHABETICAL_SEQUENCE.to_vec();
        assert_eq!(
            settings.sequence_choice(),
            SequenceChoice::Preset(PresetId::Alphabetical)
        );
        settings.curriculum.custom_sequence = vec!['K', 'M'];
        assert_eq!(settings.sequence_choice(), SequenceChoice::Custom);
        assert_eq!(PresetId::CwAcademy.preset().name, "CW Academy");
    }

    #[test]
    fn choosing_custom_keeps_the_order_that_was_showing() {
        let mut settings = TrainingSettings::default();
        settings.choose_sequence(SequenceChoice::Preset(PresetId::MorseMania));
        assert_eq!(
            settings.sequence_choice(),
            SequenceChoice::Preset(PresetId::MorseMania)
        );

        settings.choose_sequence(SequenceChoice::Custom);
        assert!(settings.curriculum.sequence_is_custom);
        assert_eq!(
            settings.curriculum.custom_sequence,
            TRADITIONAL_KOCH_SEQUENCE
        );
        // The order still matches a preset, but the user asked for custom.
        assert_eq!(settings.sequence_choice(), SequenceChoice::Custom);

        // From LCWO, custom starts from LCWO's order rather than an empty one.
        let mut settings = TrainingSettings::default();
        settings.choose_sequence(SequenceChoice::Custom);
        assert_eq!(settings.curriculum.custom_sequence, LCWO_SEQUENCE);
        assert_eq!(settings.sequence_choice(), SequenceChoice::Custom);
    }

    #[test]
    fn a_preset_click_leaves_custom_and_lcwo_clears_the_stored_order() {
        let mut settings = TrainingSettings::default();
        settings.choose_sequence(SequenceChoice::Custom);
        settings.choose_sequence(SequenceChoice::Preset(PresetId::Alphabetical));
        assert_eq!(
            settings.sequence_choice(),
            SequenceChoice::Preset(PresetId::Alphabetical)
        );
        assert!(!settings.curriculum.sequence_is_custom);

        settings.choose_sequence(SequenceChoice::Preset(PresetId::Lcwo));
        assert!(settings.curriculum.custom_sequence.is_empty());
        assert!(!settings.curriculum.sequence_is_custom);
        assert_eq!(settings.sequence(), LCWO_SEQUENCE);
    }

    /// A preset tried in place of an order the learner typed keeps theirs,
    /// and Your own brings it back, whichever presets were tried between:
    /// LCWO, which empties the order, and an emptied box, which is LCWO,
    /// keep it too.
    #[test]
    fn a_preset_tried_in_place_of_ones_own_order_keeps_it_for_your_own() {
        let own = |settings: &TrainingSettings| settings.sequence().iter().collect::<String>();
        let mut settings = TrainingSettings::default();
        settings.type_sequence("ZQXJKMRSU");
        settings.choose_sequence(SequenceChoice::Preset(PresetId::MorseMania));
        assert_eq!(settings.sequence(), TRADITIONAL_KOCH_SEQUENCE);
        settings.choose_sequence(SequenceChoice::Preset(PresetId::Lcwo));
        assert_eq!(settings.sequence(), LCWO_SEQUENCE);
        settings.choose_sequence(SequenceChoice::Custom);
        assert_eq!(settings.sequence_choice(), SequenceChoice::Custom);
        assert_eq!(own(&settings), "ZQXJKMRSU");
        assert!(settings.curriculum.kept_sequence.is_empty());

        settings.type_sequence(" ");
        assert_eq!(
            settings.sequence_choice(),
            SequenceChoice::Preset(PresetId::Lcwo)
        );
        settings.choose_sequence(SequenceChoice::Custom);
        assert_eq!(own(&settings), "ZQXJKMRSU");

        // Your own again while it is chosen leaves it as it is.
        settings.choose_sequence(SequenceChoice::Custom);
        assert_eq!(own(&settings), "ZQXJKMRSU");
    }

    /// Only an order of the learner's own is kept: a preset's, copied to
    /// Your own and left as it was, is to be had from its pill, and Your own
    /// starts from the order showing again. An order typed while a preset is
    /// chosen is their own from then on, in place of the one kept.
    #[test]
    fn only_an_order_of_ones_own_is_kept_and_the_newest_typed_wins() {
        let mut settings = TrainingSettings::default();
        settings.choose_sequence(SequenceChoice::Custom);
        settings.choose_sequence(SequenceChoice::Preset(PresetId::MorseMania));
        assert!(settings.curriculum.kept_sequence.is_empty());
        settings.choose_sequence(SequenceChoice::Custom);
        assert_eq!(settings.sequence(), TRADITIONAL_KOCH_SEQUENCE);

        settings.type_sequence("ZQX");
        settings.choose_sequence(SequenceChoice::Preset(PresetId::CwAcademy));
        settings.type_sequence("KMR");
        assert!(settings.curriculum.kept_sequence.is_empty());
        settings.choose_sequence(SequenceChoice::Preset(PresetId::Alphabetical));
        settings.choose_sequence(SequenceChoice::Custom);
        assert_eq!(settings.sequence(), ['K', 'M', 'R']);
    }

    #[test]
    fn every_preset_teaches_only_sendable_characters() {
        for preset in SEQUENCE_PRESETS {
            assert_eq!(preset.id.preset(), preset);
            assert!(!preset.sequence.is_empty(), "{:?} is empty", preset.id);
            assert!(!preset.description.is_empty());
            for ch in preset.sequence {
                assert!(
                    crate::morse::morse_for(*ch).is_some(),
                    "{:?} teaches {ch:?}, which has no Morse code",
                    preset.id
                );
            }
            let mut unique = preset.sequence.to_vec();
            unique.sort_unstable();
            unique.dedup();
            assert_eq!(
                unique.len(),
                preset.sequence.len(),
                "{:?} repeats a character",
                preset.id
            );
        }
    }
}
