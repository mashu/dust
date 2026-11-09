//! Stored settings, read as the app may use them: whatever of them still
//! reads, brought up from older builds' saves, and clamped, with what of
//! them this build cannot read kept to be written back
//! ([`load_settings_from`], [`settings_of_keys`]); and what a file holds,
//! without the mark an editor may put before it ([`unmarked`]).

use cw_core::{RF_GAIN_MAX_DB, RF_GAIN_MIN_DB, TrainingSettings};
use serde::Deserialize;
use serde_json::{Map, Value};

/// The keys a save, `raw`, holds, brought up from older builds' saves:
/// none where it is no JSON object.
fn upgraded(raw: &str) -> Option<Map<String, Value>> {
    let Ok(Value::Object(mut stored)) = serde_json::from_str::<Value>(raw) else {
        return None;
    };
    upgrade_receiver(&mut stored);
    upgrade_agc_threshold(&mut stored);
    Some(stored)
}

/// Load settings without throwing away everything for one bad field.
///
/// A strict parse fails on a single unreadable value — a hand-edited file, or a
/// field whose type changed between versions — and the user would silently get
/// factory settings back. So fall back to applying the stored keys one at a
/// time and keeping the ones that still deserialize.
///
/// Only a key this build reads can fail to, so only those are tried one at a
/// time: a few dozen, however large the save. Every other key is carried along
/// as it is ([`TrainingSettings::unread`]), and goes in with the rest at the
/// end. Each key used to be tried on top of every key kept before it, which
/// took time in the square of their number: a file of tens of thousands of
/// keys, a backup made that way, held the app up for minutes.
fn recovered(stored: &Map<String, Value>) -> TrainingSettings {
    if let Ok(settings) = TrainingSettings::deserialize(stored) {
        return settings;
    }
    let read = TrainingSettings::read_keys();
    let (tried, carried): (Vec<_>, Vec<_>) = stored
        .iter()
        .partition(|(key, _)| read.contains(key.as_str()));
    let mut kept = Map::new();
    for (key, value) in tried {
        kept.insert(key.clone(), value.clone());
        if TrainingSettings::deserialize(&kept).is_err() {
            kept.remove(key);
        }
    }
    let settings = TrainingSettings::deserialize(&kept).unwrap_or_default();
    kept.extend(
        carried
            .into_iter()
            .map(|(key, value)| (key.clone(), value.clone())),
    );
    TrainingSettings::deserialize(&kept).unwrap_or(settings)
}

/// Settings read from `raw` as [`recovered`] reads them, unclamped.
#[cfg(test)]
fn recover_settings(raw: &str) -> TrainingSettings {
    upgraded(raw).map_or_else(TrainingSettings::default, |stored| recovered(&stored))
}

/// A save from before the receiver was quoted by its 6 dB width has no
/// `filterWidthHz`. Its filter shape named a different receiver — almost
/// always the old sharp default, stored because every save stores it — so it
/// is not carried over either: such a save comes back on today's receiver,
/// shape and width both, rather than keeping one half of the old one.
fn upgrade_receiver(stored: &mut serde_json::Map<String, serde_json::Value>) {
    if !stored.contains_key("filterWidthHz") {
        stored.remove("filterShape");
    }
}

/// A save from before the AGC threshold went may have one, `agcThresholdDb`.
/// The AGC judges everything against its knee, so a threshold raised X dB
/// is the RF gain backed off X dB, but for a tenth of that in volume: such
/// a save comes back with its RF gain backed off as far, as far as the RF
/// gain goes. A threshold taken down into the hiss has nothing to come back
/// as, and nor has one under an AGC switched off, which it never moved.
fn upgrade_agc_threshold(stored: &mut serde_json::Map<String, serde_json::Value>) {
    let Some(threshold) = stored
        .remove("agcThresholdDb")
        .and_then(|threshold| threshold.as_f64())
    else {
        return;
    };
    let agc = stored
        .get("agcEnabled")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(true);
    if !agc || threshold <= 0.0 {
        return;
    }
    let rf = stored
        .get("rfGainDb")
        .and_then(serde_json::Value::as_f64)
        .filter(|db| db.is_finite())
        .unwrap_or(RF_GAIN_MAX_DB)
        .clamp(RF_GAIN_MIN_DB, RF_GAIN_MAX_DB);
    let backed_off = (rf - threshold).round().max(RF_GAIN_MIN_DB);
    stored.insert("rfGainDb".into(), serde_json::json!(backed_off));
}

/// Settings from the keys of a save, `stored`, as the app may use them:
/// whatever of them still reads, clamped, with each value this build
/// cannot read kept to be written back as it was stored, until that
/// setting is changed here ([`TrainingSettings::keeping_values_of`]).
pub(super) fn settings_of_keys(stored: &Map<String, Value>) -> TrainingSettings {
    recovered(stored).clamp().keeping_values_of(stored)
}

/// Stored settings, as the app may use them.
pub(super) fn load_settings_from(raw: &str) -> TrainingSettings {
    upgraded(raw).map_or_else(
        || TrainingSettings::default().clamp(),
        |stored| settings_of_keys(&stored),
    )
}

/// What a file holds, `raw`, without the mark of its encoding that an
/// editor saving it may put first, a byte order mark: no part of what it
/// holds, and no JSON either.
pub(super) fn unmarked(raw: &str) -> &str {
    raw.strip_prefix('\u{feff}').unwrap_or(raw)
}

#[cfg(test)]
mod tests {
    use super::*;
    use cw_core::CharSetMode;

    #[test]
    fn one_unreadable_field_does_not_reset_every_setting() {
        let mut stored = TrainingSettings::default();
        stored.playback.char_wpm_min = 12.0;
        stored.playback.char_wpm_max = 33.0;
        stored.curriculum.num_groups = 7;
        stored.band.qrn_enabled = false;
        let mut json = serde_json::to_value(&stored).unwrap();
        // Something wrote a string where a number belongs.
        json["groupTimeout"] = serde_json::json!("nonsense");
        let raw = serde_json::to_string(&json).unwrap();
        assert!(serde_json::from_str::<TrainingSettings>(&raw).is_err());

        let recovered = recover_settings(&raw);
        assert_eq!(recovered.playback.char_wpm_min, 12.0);
        assert_eq!(recovered.playback.char_wpm_max, 33.0);
        assert_eq!(recovered.curriculum.num_groups, 7);
        assert!(!recovered.band.qrn_enabled);
        // Only the unreadable field falls back to its default.
        assert_eq!(
            recovered.playback.group_timeout,
            TrainingSettings::default().playback.group_timeout
        );
    }

    /// Recovering a save takes time in proportion to its size. One bad value
    /// among a hundred thousand keys no build reads, as in a file made to
    /// hang the app, used to be tried key by key over all those kept before
    /// it: over an hour. What no build reads is carried along only so far.
    #[test]
    fn a_save_made_to_be_large_is_recovered_quickly() {
        let mut stored = serde_json::Map::new();
        for n in 0..100_000 {
            stored.insert(format!("filler{n}"), serde_json::json!(n));
        }
        stored.insert("groupTimeout".into(), serde_json::json!("nonsense"));
        stored.insert("numGroups".into(), serde_json::json!(7));
        let raw = serde_json::to_string(&stored).expect("serialize");
        let started = std::time::Instant::now();
        let settings = recover_settings(&raw);
        let took = started.elapsed();
        assert!(took < std::time::Duration::from_secs(10), "{took:?}");
        assert_eq!(settings.curriculum.num_groups, 7);
        assert_eq!(
            settings.playback.group_timeout,
            TrainingSettings::default().playback.group_timeout
        );
        let written = serde_json::to_string(&settings).expect("serialize").len();
        let fresh = serde_json::to_string(&TrainingSettings::default())
            .expect("serialize")
            .len();
        assert!(written < fresh + 20_000, "{written}");
        // Read as the stores read it, with the value no build reads kept
        // to be written back, it is as quick, and as small.
        let started = std::time::Instant::now();
        let settings = load_settings_from(&raw);
        let took = started.elapsed();
        assert!(took < std::time::Duration::from_secs(10), "{took:?}");
        let kept: Vec<&String> = settings.unread_values.keys().collect();
        assert_eq!(kept, ["groupTimeout"]);
        let written = serde_json::to_string(&settings).expect("serialize").len();
        assert!(written < fresh + 20_000, "{written}");
    }

    /// Saves from before the receiver rework all name the old sharp filter,
    /// because every save stores its shape: they come back on today's
    /// receiver. A save made since, which chose sharp, keeps it.
    #[test]
    fn a_save_from_the_old_receiver_comes_back_on_the_new_one() {
        let old = r#"{"filterBandwidthHz":500,"filterShape":"sharp","numGroups":7}"#;
        let settings = recover_settings(old);
        let fresh = TrainingSettings::default();
        assert_eq!(settings.band.filter_shape, fresh.band.filter_shape);
        assert_eq!(settings.band.filter_width_hz, fresh.band.filter_width_hz);
        assert_eq!(
            settings.curriculum.num_groups, 7,
            "the rest of the save is kept"
        );

        let mut chosen = TrainingSettings::default();
        chosen.band.filter_shape = cw_core::FilterShape::Sharp;
        chosen.band.filter_width_hz = 400.0;
        let back = recover_settings(&serde_json::to_string(&chosen).expect("serialize"));
        assert_eq!(back.band.filter_shape, cw_core::FilterShape::Sharp);
        assert_eq!(back.band.filter_width_hz, 400.0);
    }

    #[test]
    fn settings_from_an_older_build_still_load() {
        // No repeat, envelope or theme fields existed then.
        let raw =
            r#"{"level":9,"charSetMode":"koch","charWpmMin":22,"charWpmMax":22,"numGroups":15}"#;
        let settings = recover_settings(raw);
        assert_eq!(settings.curriculum.level, 9);
        assert_eq!(settings.curriculum.char_set_mode, CharSetMode::Koch);
        assert_eq!(settings.playback.char_wpm_min, 22.0);
        assert_eq!(settings.curriculum.num_groups, 15);
        assert_eq!(settings.playback.group_repeat_min, 1);
        assert_eq!(settings.playback.keyer_wpm, 20.0);
        assert_eq!(settings.playback.group_pause_sec, 0.0);
        // Callsign mode did not exist then either, so its tier starts where
        // a new one does rather than resetting everything around it.
        assert_eq!(
            settings.curriculum.callsign_level,
            cw_core::CALLSIGN_TIER_DEFAULT
        );
    }

    /// The background switch has been called `qrm*`, then `receiver*`, and is
    /// now the band noise. Every save written under either old name keeps its
    /// on/off choice. The old level and the resonator model's knobs drove a
    /// sound that no longer exists, so they are dropped rather than
    /// reinterpreted — and dropping them must not cost anything else in the
    /// save. So is the filter width of those days, `filterBandwidthHz`: a
    /// width at 3 dB where the control now means 6 dB, it comes back as
    /// today's default.
    #[test]
    fn saves_from_the_old_receiver_models_still_load() {
        let raw = r#"{"qrmEnabled":false,"qrmLevel":0.75,"qrmProfile":"ringing","charWpmMin":19}"#;
        let settings = recover_settings(raw);
        assert!(!settings.band.noise_enabled);
        assert_eq!(
            settings.band.noise_level,
            TrainingSettings::default().band.noise_level
        );
        assert_eq!(settings.playback.char_wpm_min, 19.0);

        let raw = r#"{"receiverEnabled":false,"receiverLevel":0.2,"receiverProfile":"whistle",
            "receiverBackgroundGain":55,"receiverBackgroundResonance":120,
            "receiverBackgroundOffsetModDepthHz":45,"filterBandwidthHz":250,"qrnLevel":0.4}"#;
        let settings = recover_settings(raw);
        assert!(!settings.band.noise_enabled);
        assert_eq!(
            settings.band.filter_width_hz,
            cw_core::DEFAULT_FILTER_WIDTH_HZ
        );
        assert_eq!(settings.band.qrn_level, 0.4);
        assert_eq!(settings.band.filter_shape, cw_core::FilterShape::Sharp);

        // And the new ones round-trip.
        let mut fresh = TrainingSettings::default();
        fresh.band.filter_shape = cw_core::FilterShape::Soft;
        fresh.band.filter_width_hz = 250.0;
        fresh.band.noise_level = 0.8;
        let back = recover_settings(&serde_json::to_string(&fresh).expect("serialize"));
        assert_eq!(back.band.filter_shape, cw_core::FilterShape::Soft);
        assert_eq!(back.band.filter_width_hz, 250.0);
        assert_eq!(back.band.noise_level, 0.8);
    }

    /// A save written in callsign mode has to come back in callsign mode, tier
    /// and all — this is the field a user would lose on every restart if the
    /// serde default were wrong.
    #[test]
    fn a_callsign_save_comes_back_as_one() {
        let mut settings = TrainingSettings::default();
        settings.curriculum.char_set_mode = CharSetMode::Callsign;
        settings.curriculum.callsign_level = 5;
        settings.curriculum.level = 12;
        let raw = serde_json::to_string(&settings).expect("settings serialize");
        let back = recover_settings(&raw);
        assert_eq!(back.curriculum.char_set_mode, CharSetMode::Callsign);
        assert_eq!(back.curriculum.callsign_level, 5);
        assert_eq!(back.curriculum.level, 12, "the Koch level rides along");
        assert_eq!(back.active_level(), 5);
    }

    #[test]
    fn unreadable_settings_fall_back_to_defaults() {
        assert_eq!(recover_settings("not json"), TrainingSettings::default());
        assert_eq!(recover_settings("[]"), TrainingSettings::default());
        assert_eq!(recover_settings(""), TrainingSettings::default());
    }

    #[test]
    fn loaded_settings_are_clamped_and_fitted() {
        let mut stored = TrainingSettings::default();
        stored.playback.char_wpm_min = 500.0;
        stored.curriculum.num_groups = 0;
        let settings = load_settings_from(&serde_json::to_string(&stored).unwrap());
        assert_eq!(settings.playback.char_wpm_min, 80.0);
        assert_eq!(settings.curriculum.num_groups, 1);
    }

    /// A save a later build wrote: a filter shape, an AGC, a character set,
    /// a practice window and a contest this build has no name for, with
    /// that contest's callers' styles, and a speed past this build's range.
    const LATER_SAVE: &str = r#"{
        "filterShape": "notch", "filterWidthHz": 300, "agcDecay": "medium",
        "charSetMode": "morse", "practiceWindow": "last7", "contestKind": "cwt",
        "callerStyles": {"cwt": {"at": [0.25, -0.5], "off": []}},
        "charWpmMax": 95, "numGroups": 7
    }"#;

    /// The keys of `LATER_SAVE` this build cannot read, as it stored them.
    fn unreadable_in_later_save() -> serde_json::Map<String, serde_json::Value> {
        let serde_json::Value::Object(save) = serde_json::from_str(LATER_SAVE).expect("JSON")
        else {
            unreachable!("an object")
        };
        save.into_iter()
            .filter(|(key, _)| {
                !matches!(key.as_str(), "filterWidthHz" | "charWpmMax" | "numGroups")
            })
            .collect()
    }

    /// Each value of a save this build cannot read, under a key it reads,
    /// is read as this build can, and goes back as it was stored, so the
    /// later build that wrote it still finds it. A number past this
    /// build's range is not one: it is read, held in the range, as before.
    /// Each used to go back as what this build read it as.
    #[test]
    fn a_value_this_build_cannot_read_goes_back_as_it_was_stored() {
        let settings = load_settings_from(LATER_SAVE);
        assert_eq!(settings.band.filter_shape, cw_core::FilterShape::Sharp);
        assert_eq!(settings.band.agc_decay, cw_core::AgcDecay::Fast);
        assert_eq!(settings.curriculum.char_set_mode, CharSetMode::Mixed);
        assert_eq!(
            settings.curriculum.practice_window,
            cw_core::PracticeWindow::All
        );
        assert_eq!(settings.contest.contest_kind, cw_core::ContestKind::Wpx);
        assert!(settings.contest.caller_styles.is_empty());
        assert_eq!(settings.playback.char_wpm_max, 80.0);
        assert_eq!(settings.curriculum.num_groups, 7);

        let written = serde_json::to_value(&settings).expect("serialize");
        for (key, stored) in unreadable_in_later_save() {
            assert_eq!(written[&key], stored, "{key} goes back as it was stored");
        }
        assert_eq!(written["charWpmMax"], 80.0, "held in this build's range");
        assert_eq!(written["numGroups"], 7);
        // Read again, it is the same.
        let again = load_settings_from(&serde_json::to_string(&settings).expect("serialize"));
        assert_eq!(again, settings);
    }

    /// A value this build cannot read goes back as it was stored until the
    /// setting is changed here: from then on, as it now is, chosen here,
    /// though it is chosen back.
    #[test]
    fn a_setting_changed_here_goes_as_it_now_is() {
        let mut settings = load_settings_from(LATER_SAVE);
        settings.band.filter_shape = cw_core::FilterShape::Soft;
        settings.playback.char_wpm_max = 40.0;
        let settings = settings.clamp();
        let written = serde_json::to_value(&settings).expect("serialize");
        assert_eq!(written["filterShape"], "soft");
        assert_eq!(written["charWpmMax"], 40.0);
        for (key, stored) in unreadable_in_later_save() {
            if key != "filterShape" {
                assert_eq!(written[&key], stored, "{key} goes back as it was stored");
            }
        }
        let again = load_settings_from(&serde_json::to_string(&settings).expect("serialize"));
        assert_eq!(again, settings, "the same, read again");

        // Chosen back, it is this build's choice.
        let mut back = settings;
        back.band.filter_shape = cw_core::FilterShape::Sharp;
        let written = serde_json::to_value(back.clamp()).expect("serialize");
        assert_eq!(written["filterShape"], "sharp");
        assert_eq!(written["agcDecay"], "medium", "the rest as stored");
    }
}
