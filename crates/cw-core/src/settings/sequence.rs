use super::TrainingSettings;
use crate::morse::LCWO_SEQUENCE;
use crate::sequences::{PresetId, SEQUENCE_PRESETS, SequenceChoice};

impl TrainingSettings {
    /// The unlock order the settings hold.
    ///
    /// A save says this in two fields: the order itself, empty for LCWO's,
    /// and whether the user picked Custom. This is the one place they are
    /// read together.
    pub fn sequence_choice(&self) -> SequenceChoice {
        let curriculum = &self.curriculum;
        if curriculum.sequence_is_custom {
            return SequenceChoice::Custom;
        }
        if curriculum.custom_sequence.is_empty() {
            return SequenceChoice::Preset(PresetId::Lcwo);
        }
        SEQUENCE_PRESETS
            .iter()
            .find(|preset| preset.sequence == curriculum.custom_sequence)
            .map_or(SequenceChoice::Custom, |preset| {
                SequenceChoice::Preset(preset.id)
            })
    }

    /// Choose an unlock order. A preset chosen in place of an order the
    /// learner typed keeps theirs aside, and Custom brings it back: trying
    /// a preset should not cost what took typing. With none kept, Custom
    /// starts from the order showing, so the user edits what they were
    /// looking at.
    pub fn choose_sequence(&mut self, choice: SequenceChoice) {
        let was_custom = self.sequence_choice() == SequenceChoice::Custom;
        let curriculum = &mut self.curriculum;
        match choice {
            SequenceChoice::Custom => {
                if !was_custom && !curriculum.kept_sequence.is_empty() {
                    curriculum.custom_sequence = std::mem::take(&mut curriculum.kept_sequence);
                } else if curriculum.custom_sequence.is_empty() {
                    curriculum.custom_sequence = LCWO_SEQUENCE.to_vec();
                }
                curriculum.sequence_is_custom = true;
            }
            SequenceChoice::Preset(id) => {
                // A preset's order, copied to Custom and left as it was, is
                // to be had from its pill: only an order of their own is
                // kept.
                let typed = !SEQUENCE_PRESETS
                    .iter()
                    .any(|preset| preset.sequence == curriculum.custom_sequence);
                if was_custom && typed {
                    curriculum.kept_sequence = std::mem::take(&mut curriculum.custom_sequence);
                }
                curriculum.custom_sequence = match id {
                    // LCWO's is the empty order, as saves have always had it.
                    PresetId::Lcwo => Vec::new(),
                    _ => id.preset().sequence.to_vec(),
                };
                curriculum.sequence_is_custom = false;
            }
        }
        *self = std::mem::take(self).clamp();
    }

    /// Take an unlock order the learner typed out. Only what can be sent
    /// counts, each character once. A preset's own order is that preset,
    /// however it got there, and nothing at all is LCWO's, the order an empty
    /// sequence has always meant; anything else is the learner's own, in
    /// place of any kept.
    pub fn type_sequence(&mut self, typed: &str) {
        let order = Self::unique_alphabet(&typed.chars().collect::<Vec<_>>());
        if order.is_empty() {
            return self.choose_sequence(SequenceChoice::Preset(PresetId::Lcwo));
        }
        if let Some(preset) = SEQUENCE_PRESETS.iter().find(|p| p.sequence == order) {
            return self.choose_sequence(SequenceChoice::Preset(preset.id));
        }
        self.curriculum.custom_sequence = order;
        self.curriculum.sequence_is_custom = true;
        self.curriculum.kept_sequence.clear();
    }
}
