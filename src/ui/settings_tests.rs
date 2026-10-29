//! The settings screen in the whole app: what a learner types and clicks,
//! and what the screen holds afterwards.
//!
//! These read the page rather than the store, so they say what the learner
//! sees; the store is written from the same settings the page shows.

use cw_core::{Aim, CharSetMode, Experience, RangeSetting, RangeValues};

use crate::persist::Store;
use crate::testing::{Ui, run, test_settings};

/// The value an input is showing.
fn value_of(ui: &Ui, id: &str) -> String {
    let tag = ui.input_tag(id);
    tag.split("value=\"")
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .unwrap_or_else(|| panic!("{id} shows no value: {tag}"))
        .to_string()
}

/// A number is typed through its prefixes. Retyping the top of a range from
/// 25 to 30 passes through 3, and that must not drag the bottom down with it.
#[test]
fn retyping_one_end_of_a_range_leaves_the_other_alone() {
    run(|| async {
        let mut settings = test_settings();
        settings.playback.char_wpm_min = 20.0;
        settings.playback.char_wpm_max = 25.0;
        let (mut ui, _recorder) = Ui::app_with_settings(settings);
        ui.click("nav-settings");
        ui.type_into("to-character-speed", "3");
        ui.type_into("to-character-speed", "30");
        ui.commit("to-character-speed", "30");
        ui.advance(50).await;
        assert_eq!(value_of(&ui, "from-character-speed"), "20");
        assert_eq!(value_of(&ui, "to-character-speed"), "30");
        assert!(ui.has("20–30 WPM"), "{}", ui.html());
    });
}

/// The side tone has no stored link: a range whose ends meet is a fixed
/// pitch. The first digit of a new top used to make them meet, which swapped
/// the two boxes for one and took away the box being typed in.
#[test]
fn typing_into_one_end_of_the_side_tone_keeps_its_box() {
    run(|| async {
        let (mut ui, _recorder) = Ui::app_with_settings(test_settings());
        ui.click("nav-settings");
        ui.type_into("to-side-tone", "4");
        assert!(ui.shows("to-side-tone"), "the box being typed in went away");
        assert!(!ui.shows("fixed-side-tone"));
        ui.type_into("to-side-tone", "700");
        ui.commit("to-side-tone", "700");
        ui.advance(50).await;
        assert_eq!(value_of(&ui, "from-side-tone"), "300");
        assert_eq!(value_of(&ui, "to-side-tone"), "700");
    });
}

/// Fixed, then Random, opens a range to draw from on every range the page
/// shows, so that Random does something at once, and a note that asks for
/// a range is answered by the press. Each used to stay at its one value
/// but the side tone and the stations.
#[test]
fn random_opens_every_fixed_value_into_a_range() {
    run(|| async {
        let (mut ui, _recorder) = Ui::app_with_settings(test_settings());
        ui.click("nav-settings");
        ui.commit("switch-tie-effective-speed-to-character-speed", "false");
        ui.click("link-group-size");
        assert!(ui.has("Set Group size to Random to use this"));
        for name in [
            "character-speed",
            "effective-speed",
            "group-size",
            "sends-per-group",
            "side-tone",
            "station-strength",
            "stations-calling",
        ] {
            let link = format!("link-{name}");
            if !ui.shows(&format!("fixed-{name}")) {
                ui.click(&link);
            }
            assert!(ui.shows(&format!("fixed-{name}")), "{name} is not fixed");
            ui.click(&link);
            let (from, to) = (format!("from-{name}"), format!("to-{name}"));
            assert_ne!(value_of(&ui, &from), value_of(&ui, &to), "{name}");
        }
        assert_eq!(value_of(&ui, "from-group-size"), "2");
        assert_eq!(value_of(&ui, "to-group-size"), "3");
        assert!(!ui.has("Set Group size to Random to use this"));
    });
}

/// A number committed past what the field allows comes back as the most it
/// allows, even when that is what the setting already was. Until then the
/// 999 is the box's own: nothing is written over it while it is typed.
#[test]
fn a_number_committed_out_of_bounds_shows_where_it_landed() {
    run(|| async {
        let mut settings = test_settings();
        settings.playback.group_pause_sec = cw_core::GROUP_PAUSE_MAX_SEC;
        let (mut ui, _recorder) = Ui::app_with_settings(settings);
        ui.click("nav-settings");
        let field = "number-pause-after-each-group";
        let _ = ui.take_work();
        ui.type_into(field, "999");
        assert_eq!(ui.take_work(), 0, "typing wrote over the box");
        ui.commit(field, "999");
        // The setting did not move, so only the field itself can write the
        // most it allows back over the 999.
        assert!(ui.take_work() > 0, "the commit left 999 in the box");
        assert_eq!(
            value_of(&ui, field),
            cw_core::GROUP_PAUSE_MAX_SEC.to_string()
        );
        ui.click("less-number-pause-after-each-group");
        assert_eq!(
            value_of(&ui, field),
            (cw_core::GROUP_PAUSE_MAX_SEC - 0.5).to_string()
        );
    });
}

/// Type `text` into `field` a key at a time, faster than the page draws, and
/// give back what the box on screen holds at the end.
///
/// On the desktop and phone builds a redraw reaches the box a frame after it
/// was made, and it sends the box its `value` whether or not that changed. A
/// field that redrew for every key wrote each key's text back over whatever
/// had been typed since. The harness draws at once, so this plays the
/// webview's part, at its worst: every redraw lands a key late.
fn type_faster_than_the_page_draws(ui: &mut Ui, field: &str, text: &str) -> String {
    let _ = ui.take_work();
    // What the box on screen holds, and a redraw still on its way to it.
    let mut screen = String::new();
    let mut on_its_way: Option<String> = None;
    for key in text.chars() {
        screen.push(key);
        ui.type_into(field, &screen);
        if let Some(late) = on_its_way.take() {
            screen = late;
        }
        if ui.take_work() > 0 {
            on_its_way = Some(value_of(ui, field));
        }
    }
    if let Some(late) = on_its_way {
        screen = late;
    }
    screen
}

/// A number typed faster than the page draws is committed whole: a field
/// that redrew for every key turned a quick 120 into 10.
#[test]
fn a_number_typed_faster_than_the_page_draws_is_committed_whole() {
    run(|| async {
        let (mut ui, _recorder) = Ui::app_with_settings(test_settings());
        ui.click("nav-settings");
        let field = "number-groups-per-session";
        let screen = type_faster_than_the_page_draws(&mut ui, field, "120");
        assert_eq!(screen, "120");
        assert_eq!(
            ui.stored_settings().await.curriculum.num_groups,
            2,
            "nothing is taken before it is committed"
        );
        ui.commit(field, &screen);
        assert_eq!(value_of(&ui, field), "120");
        assert_eq!(ui.stored_settings().await.curriculum.num_groups, 120);
    });
}

/// An unlock order typed faster than the page draws is committed whole. Its
/// box read what was typed as it drew, so every key redrew it, and a quick
/// KMURESN came out as KMUREN.
#[test]
fn an_unlock_order_typed_faster_than_the_page_draws_is_committed_whole() {
    run(|| async {
        let (mut ui, _recorder) = Ui::app_with_settings(test_settings());
        ui.click("nav-settings");
        let field = "field-sequence-order";
        let screen = type_faster_than_the_page_draws(&mut ui, field, "KMURESN");
        assert_eq!(screen, "KMURESN");
        ui.commit(field, &screen);
        assert_eq!(value_of(&ui, field), "KMURESN");
        let stored = ui.stored_settings().await.curriculum.custom_sequence;
        assert_eq!(stored.iter().collect::<String>(), "KMURESN");
    });
}

/// A callsign typed faster than the page draws is committed whole, as every
/// other box the settings are typed into is.
#[test]
fn a_callsign_typed_faster_than_the_page_draws_is_committed_whole() {
    run(|| async {
        let (mut ui, _recorder) = Ui::app_with_settings(crate::testing::contest_settings());
        ui.click("nav-settings");
        let field = "field-your-callsign";
        let screen = type_faster_than_the_page_draws(&mut ui, field, "VP2E/DL1ABC");
        assert_eq!(screen, "VP2E/DL1ABC");
        ui.commit(field, &screen);
        assert_eq!(value_of(&ui, field), "VP2E/DL1ABC");
        assert_eq!(ui.stored_settings().await.contest.my_call, "VP2E/DL1ABC");
    });
}

/// A fraction typed into a whole-number field is rounded to the nearest
/// whole number, as a range's ends are, rather than cut off: 2.9 groups
/// used to be 2, while 2.9 characters a group was 3.
#[test]
fn a_fraction_typed_into_a_whole_number_rounds() {
    run(|| async {
        let (mut ui, _recorder) = Ui::app_with_settings(test_settings());
        ui.click("nav-settings");
        ui.commit("number-groups-per-session", "2.9");
        ui.commit("number-countdown-before-the-first-group", "2.6");
        ui.commit("to-group-size", "4.7");
        let stored = ui.stored_settings().await;
        assert_eq!(stored.curriculum.num_groups, 3);
        assert_eq!(stored.playback.countdown_sec, 3);
        assert_eq!(stored.curriculum.max_group_size, 5);
        assert_eq!(value_of(&ui, "number-groups-per-session"), "3");
    });
}

/// − and + move the box as well as the setting, before anything is typed
/// and after.
#[test]
fn the_step_buttons_move_the_number_in_the_box() {
    run(|| async {
        let (mut ui, _recorder) = Ui::app_with_settings(test_settings());
        ui.click("nav-settings");
        let field = "number-groups-per-session";
        ui.click("more-number-groups-per-session");
        assert_eq!(value_of(&ui, field), "3");
        ui.type_into(field, "12");
        ui.commit(field, "12");
        assert_eq!(value_of(&ui, field), "12");
        ui.click("more-number-groups-per-session");
        assert_eq!(value_of(&ui, field), "13");
        ui.click("less-number-groups-per-session");
        ui.click("less-number-groups-per-session");
        assert_eq!(value_of(&ui, field), "11");
        assert_eq!(ui.stored_settings().await.curriculum.num_groups, 11);
    });
}

/// − and + show the number they step to over text that was typed and never
/// committed.
///
/// A key typed straight after Enter can be written over by the commit's
/// redraw, a frame late. The box is then back as it was committed, so
/// leaving it commits nothing, but the field still holds the text with the
/// key in it. A step drew that text back over the new number: the box said
/// 405 while the setting had gone to 41.
#[test]
fn a_step_shows_the_number_stepped_to_over_text_never_committed() {
    run(|| async {
        let (mut ui, _recorder) = Ui::app_with_settings(test_settings());
        ui.click("nav-settings");
        let field = "number-groups-per-session";
        ui.type_into(field, "40");
        ui.commit(field, "40");
        // The 5, which the commit's redraw wrote over.
        ui.type_into(field, "405");
        ui.click("more-number-groups-per-session");
        assert_eq!(value_of(&ui, field), "41");
        assert_eq!(ui.stored_settings().await.curriculum.num_groups, 41);
        // A phone can keep the box focused through a tap on −, so what is
        // typed before it is never committed either.
        ui.type_into(field, "7");
        ui.click("less-number-groups-per-session");
        assert_eq!(value_of(&ui, field), "40");
        assert_eq!(ui.stored_settings().await.curriculum.num_groups, 40);
    });
}

/// A step up to the field's top shows the top, not the text typed past it.
#[test]
fn a_step_to_the_top_shows_the_top_over_text_never_committed() {
    run(|| async {
        let mut settings = test_settings();
        settings.curriculum.num_groups = cw_core::NUM_GROUPS_MAX - 1;
        let (mut ui, _recorder) = Ui::app_with_settings(settings);
        ui.click("nav-settings");
        let field = "number-groups-per-session";
        let more = "more-number-groups-per-session";
        ui.type_into(field, "1995");
        ui.click(more);
        assert_eq!(value_of(&ui, field), cw_core::NUM_GROUPS_MAX.to_string());
        assert!(
            ui.input_tag(more).contains("disabled"),
            "{}",
            ui.input_tag(more)
        );
        assert_eq!(
            ui.stored_settings().await.curriculum.num_groups,
            cw_core::NUM_GROUPS_MAX
        );
    });
}

/// One click to look at another character set, or a short order of one's
/// own, must not cost the level the learner had reached.
#[test]
fn trying_another_character_set_costs_no_level() {
    run(|| async {
        let mut settings = test_settings();
        settings.curriculum.level = 38;
        let (mut ui, _recorder) = Ui::app_with_settings(settings);
        ui.click("nav-settings");
        ui.click("seg-mixed");
        ui.click("seg-koch");
        ui.advance(50).await;
        assert_eq!(value_of(&ui, "field-level"), "38");

        let mut settings = test_settings();
        settings.curriculum.level = 20;
        let (mut ui, _recorder) = Ui::app_with_settings(settings);
        ui.click("nav-settings");
        ui.commit("field-sequence-order", "ABC");
        assert_eq!(value_of(&ui, "field-level"), "2");
        ui.click("pill-lcwo");
        ui.advance(50).await;
        assert_eq!(value_of(&ui, "field-level"), "20");
        ui.click("nav-practice");
        assert!(ui.has("Level 20"), "{}", ui.html());
    });
}

/// The unlock order can be emptied and typed out again. It is only read
/// once it is committed, so an empty box is not refilled with LCWO's order
/// under the cursor, and typing LCWO's order back is LCWO again.
#[test]
fn the_unlock_order_can_be_cleared_and_typed_again() {
    run(|| async {
        let mut settings = test_settings();
        settings.curriculum.level = 20;
        let (mut ui, _recorder) = Ui::app_with_settings(settings);
        ui.click("nav-settings");
        // Emptied and typed into, the box is left as typed: nothing is
        // written back over it until it is committed.
        let _ = ui.take_work();
        ui.type_into("field-sequence-order", "");
        assert_eq!(ui.take_work(), 0, "the emptied box was written over");
        ui.type_into("field-sequence-order", "KM");
        assert_eq!(ui.take_work(), 0, "typing wrote over the box");
        assert_eq!(value_of(&ui, "field-level"), "20", "nothing is read yet");

        let lcwo: String = cw_core::SEQUENCE_PRESETS[0].sequence.iter().collect();
        ui.type_into("field-sequence-order", &lcwo);
        ui.commit("field-sequence-order", &lcwo);
        ui.advance(50).await;
        assert_eq!(value_of(&ui, "field-sequence-order"), lcwo);
        assert_eq!(value_of(&ui, "field-level"), "20");
        assert!(ui.input_tag("pill-lcwo").contains("active"));
        assert!(!ui.input_tag("pill-your-own").contains("active"));

        // Committed with stray characters, it is cleaned once, on commit.
        ui.commit("field-sequence-order", "km k?");
        assert_eq!(value_of(&ui, "field-sequence-order"), "KM?");
        assert!(ui.input_tag("pill-your-own").contains("active"));
    });
}

/// A list of one's own is an unlock order of one's own: typed into the
/// order's box, or started from a preset with "Your own". There is no
/// character set beside it called Custom to do the same.
#[test]
fn a_list_of_ones_own_is_an_unlock_order() {
    run(|| async {
        let mut settings = test_settings();
        settings.curriculum.char_set_mode = CharSetMode::Koch;
        settings.curriculum.level = 6;
        let (mut ui, _recorder) = Ui::app_with_settings(settings);
        ui.click("nav-settings");
        assert!(!ui.shows("seg-custom"));

        ui.click("pill-morsemania");
        assert!(ui.has("Unlocked in MorseMania order · 7 in play"));
        ui.click("pill-your-own");
        assert!(ui.input_tag("pill-your-own").contains("active"));
        assert_eq!(
            value_of(&ui, "field-sequence-order"),
            "KMRSUAPTLOWINJEF0YVG5Q9ZH38B?427C16DX/=+",
            "starts from the order showing"
        );

        ui.commit("field-sequence-order", "qrstuvwxyz");
        ui.advance(50).await;
        assert_eq!(value_of(&ui, "field-sequence-order"), "QRSTUVWXYZ");
        assert_eq!(value_of(&ui, "field-level"), "6");
        assert!(ui.has("Unlocked in your own order · 7 in play"));
        assert_eq!(
            ui.stored_settings().await.curriculum.custom_sequence,
            "QRSTUVWXYZ".chars().collect::<Vec<_>>()
        );
    });
}

/// Tap the dit paddle `times` times, a letter space apart: that many Es.
/// Keys go to the focused control and bubble up to the app's root.
async fn tap_dits(ui: &mut Ui, on: &str, times: usize) {
    use dioxus::prelude::{Code, Key};
    for _ in 0..times {
        ui.press_key(on, Key::Character("[".into()), Code::BracketLeft);
        ui.advance(30).await;
        ui.release_key(on, Key::Character("[".into()), Code::BracketLeft);
        ui.advance(250).await;
    }
}

/// The practice pad shows what the paddles key on Settings, and starts
/// clean whenever Settings is opened again.
#[test]
fn the_practice_pad_shows_what_was_keyed_until_settings_is_left() {
    run(|| async {
        let (mut ui, _recorder) = Ui::app_with_settings(test_settings());
        ui.click("nav-settings");
        tap_dits(&mut ui, "seg-iambic-a", 2).await;
        assert!(ui.has(">EE<"), "{}", ui.html());
        ui.click("nav-practice");
        tap_dits(&mut ui, "btn-start-training", 1).await;
        ui.click("nav-settings");
        assert!(!ui.has(">EE<") && !ui.has(">E<"), "{}", ui.html());
        assert!(ui.has("· −"));
    });
}

/// The paddles swapped under a held `[` leave it keying the dit it closed:
/// its next press, which tao on Linux does not mark as a repeat, reads as
/// the dah but is a repeat, and its one release opens the dit. The pad
/// hears an E, rather than dahs keyed on and on.
#[test]
fn swapping_the_paddles_under_a_held_key_leaves_none_down() {
    run(|| async {
        use dioxus::prelude::{Code, Key};
        let (mut ui, _recorder) = Ui::app_with_settings(test_settings());
        ui.click("nav-settings");
        let bracket = Key::Character("[".into());
        let on = "seg-iambic-a";
        ui.press_key(on, bracket.clone(), Code::BracketLeft);
        ui.advance(10).await;
        ui.commit("switch-swap-and-paddles", "true");
        assert!(ui.input_tag("switch-swap-and-paddles").contains("checked"));
        ui.press_key(on, bracket.clone(), Code::BracketLeft);
        ui.advance(10).await;
        ui.release_key(on, bracket, Code::BracketLeft);
        ui.advance(1_000).await;
        assert!(ui.has(">E<"), "{}", ui.html());
    });
}

/// A setting that moves redraws its own card, not the page. Every card used
/// to read the whole of the settings, so one slider tick redrew all of them
/// — about thirty mutations a tick, while being dragged.
#[test]
fn a_slider_tick_redraws_its_own_card_only() {
    run(|| async {
        let (mut ui, _recorder) = Ui::app_with_settings(test_settings());
        ui.click("nav-settings");
        ui.advance(50).await;
        for (control, most) in [
            ("more-slider-qrn-intensity", 10),
            ("more-number-key-speed", 6),
            ("more-slider-rise-time", 16),
        ] {
            let _ = ui.take_work();
            ui.click(control);
            let work = ui.take_work();
            assert!(
                work <= most,
                "{control} cost {work} mutations; its card alone is at most {most}"
            );
        }
    });
}

/// Adaptive group size is off until asked for. What it aims for shows only
/// once it is on, both are stored, and a group size of one length says why
/// there is nothing to adapt.
#[test]
fn adaptive_group_size_is_opt_in_and_its_target_follows_the_switch() {
    run(|| async {
        let mut settings = test_settings();
        settings.curriculum.max_group_size = 6;
        let (mut ui, _recorder) = Ui::app_with_settings(settings);
        ui.click("nav-settings");
        assert!(ui.shows("switch-adapt-group-size-to-me"));
        assert!(ui.has("Your copy span is under Stats."));
        assert!(!ui.shows("seg-balanced"), "no target until it is on");

        ui.commit("switch-adapt-group-size-to-me", "true");
        assert!(ui.has("Balanced aims for 70% of groups copied whole."));
        ui.click("seg-stretch");
        assert!(ui.has("Stretch aims for 50% of groups copied whole."));
        let stored = ui.stored_settings().await;
        assert!(stored.curriculum.adaptive_group_size);
        assert_eq!(stored.curriculum.aim, Aim::Stretch);

        ui.click("link-group-size");
        assert!(ui.has("one size leaves nothing to adapt"));
        assert!(!ui.shows("seg-stretch"));
    });
}

/// Adaptive speed is off until asked for. On, it shows what it aims for: in
/// the Speed card while group size does not adapt, and once it does, under
/// Session shape, one aim for both. One speed leaves nothing to adapt.
#[test]
fn adaptive_speed_is_opt_in_and_shares_its_aim_with_group_size() {
    run(|| async {
        let mut settings = test_settings();
        settings.curriculum.max_group_size = 6;
        settings.playback.char_wpm_min = 18.0;
        settings.playback.char_wpm_max = 30.0;
        settings.playback.link_char_wpm = false;
        let (mut ui, _recorder) = Ui::app_with_settings(settings);
        ui.click("nav-settings");
        assert!(ui.shows("switch-adapt-speed-to-me"));
        assert!(ui.has("Your copy speed is under Stats."));
        assert!(!ui.shows("seg-balanced"), "no target until it is on");

        ui.commit("switch-adapt-speed-to-me", "true");
        assert!(ui.has("Balanced aims for 94% of characters right."));
        ui.click("seg-comfortable");
        assert!(ui.has("Comfortable aims for 97% of characters right."));
        let stored = ui.stored_settings().await;
        assert!(stored.playback.adaptive_speed);
        assert_eq!(stored.curriculum.aim, Aim::Comfortable);

        ui.commit("switch-adapt-group-size-to-me", "true");
        assert!(
            ui.has("Comfortable aims for 85% of groups copied whole and 97% of characters right.")
        );
        assert!(ui.has("Aim for is set under Session shape"));
        assert_eq!(
            ui.controls("seg-comfortable").len(),
            1,
            "one aim on the page"
        );

        ui.click("link-character-speed");
        assert!(ui.has("one speed leaves nothing to adapt"));
    });
}

/// A callsign is as long as it is, so there is no size to adapt and no switch.
#[test]
fn callsigns_have_no_adaptive_group_size() {
    run(|| async {
        let mut settings = test_settings();
        settings.curriculum.char_set_mode = CharSetMode::Callsign;
        let (mut ui, _recorder) = Ui::app_with_settings(settings);
        ui.click("nav-settings");
        assert!(!ui.shows("switch-adapt-group-size-to-me"));
    });
}

/// The experience dropdown's markup.
fn experience_picker(ui: &Ui) -> String {
    ui.html()
        .split("id=\"select-experience\"")
        .nth(1)
        .and_then(|rest| rest.split("</select>").next())
        .expect("the experience dropdown is on the page")
        .to_string()
}

/// The option the experience dropdown shows as chosen.
fn chosen_experience(ui: &Ui) -> String {
    experience_picker(ui)
        .split("<option")
        .find(|option| option.contains("selected"))
        .and_then(|option| option.split('>').nth(1))
        .and_then(|text| text.split('<').next())
        .unwrap_or_default()
        .to_string()
}

/// Choosing an experience sets the group size and the speed, through the
/// same controls the rest of the page shows: their values follow at once.
/// The level and the session's length are the learner's own, and stay
/// where they were.
#[test]
fn choosing_an_experience_sets_the_trainer_up_for_it() {
    run(|| async {
        let mut settings = test_settings();
        settings.curriculum.level = 6;
        settings.curriculum.num_groups = 33;
        let (mut ui, _recorder) = Ui::app_with_settings(settings);
        ui.click("nav-settings");
        assert_eq!(chosen_experience(&ui), "Custom");
        assert!(ui.has("Sets group size and speed to start from."));

        ui.commit("select-experience", "intermediate");
        assert_eq!(chosen_experience(&ui), "Intermediate");
        assert!(
            !experience_picker(&ui).contains(">Custom<"),
            "Custom is offered over a preset"
        );
        assert!(ui.has("Groups of 2–3 · 15–25 WPM"), "{}", ui.html());
        assert_eq!(value_of(&ui, "from-group-size"), "2");
        assert_eq!(value_of(&ui, "to-group-size"), "3");
        assert_eq!(value_of(&ui, "from-character-speed"), "15");
        assert_eq!(value_of(&ui, "to-character-speed"), "25");
        assert_eq!(
            value_of(&ui, "number-groups-per-session"),
            "33",
            "a preset moved the session's length"
        );
        assert_eq!(
            value_of(&ui, "field-level"),
            "6",
            "a preset moved the level"
        );
        let stored = ui.stored_settings().await;
        assert_eq!(Experience::of(&stored), Some(Experience::Intermediate));
        assert_eq!(stored.curriculum.num_groups, 33);

        // A fixed size and speed show as one value each.
        ui.commit("select-experience", "novice");
        assert!(ui.has("Groups of 2 · 15 WPM"), "{}", ui.html());
        assert_eq!(value_of(&ui, "fixed-group-size"), "2");
        assert_eq!(value_of(&ui, "fixed-character-speed"), "15");
        assert_eq!(
            value_of(&ui, "field-level"),
            "6",
            "a preset moved the level"
        );
        let stored = ui.stored_settings().await;
        assert_eq!(
            stored.range(RangeSetting::CharWpm),
            RangeValues {
                min: 15.0,
                max: 15.0,
                linked: true
            }
        );
    });
}

/// Every way of changing what a preset set makes the settings your own:
/// typing, stepping, fixing or opening a range, putting a card back to its
/// defaults. Changing what it leaves alone, the level and the session's
/// length among them, does not.
#[test]
fn any_change_to_what_a_preset_set_shows_the_settings_as_your_own() {
    run(|| async {
        let edits: [(&str, &str, Option<&str>); 9] = [
            ("intermediate", "from-group-size", Some("1")),
            ("intermediate", "to-group-size", Some("4")),
            ("intermediate", "link-group-size", None),
            ("intermediate", "from-character-speed", Some("16")),
            ("expert", "to-character-speed", Some("30")),
            ("expert", "less-to-character-speed", None),
            ("expert", "link-character-speed", None),
            ("novice", "link-group-size", None),
            ("novice", "reset-speed", None),
        ];
        for (preset, control, typed) in edits {
            let (mut ui, _recorder) = Ui::app_with_settings(test_settings());
            ui.click("nav-settings");
            ui.commit("select-experience", preset);
            assert_ne!(chosen_experience(&ui), "Custom", "{preset}");
            match typed {
                Some(value) => ui.commit(control, value),
                None => ui.click(control),
            }
            assert_eq!(
                chosen_experience(&ui),
                "Custom",
                "{control} on {preset} left the preset chosen"
            );
            assert!(
                ui.has("Sets group size and speed to start from."),
                "{control} on {preset}"
            );
        }

        let (mut ui, _recorder) = Ui::app_with_settings(test_settings());
        ui.click("nav-settings");
        ui.commit("select-experience", "expert");
        ui.commit("field-level", "20");
        ui.commit("number-pause-after-each-group", "2");
        ui.commit("number-groups-per-session", "30");
        ui.click("more-number-groups-per-session");
        assert_eq!(
            chosen_experience(&ui),
            "Expert",
            "a setting it left alone took it off"
        );
        // Picked again, it puts back what was changed, and only that.
        ui.commit("to-character-speed", "30");
        assert_eq!(chosen_experience(&ui), "Custom");
        ui.commit("select-experience", "expert");
        assert_eq!(chosen_experience(&ui), "Expert");
        assert_eq!(value_of(&ui, "to-character-speed"), "35");
        assert_eq!(value_of(&ui, "number-groups-per-session"), "31");
        assert_eq!(value_of(&ui, "number-pause-after-each-group"), "2");
    });
}

/// The word box's line under it.
fn word_report(ui: &Ui) -> String {
    ui.html()
        .split("class=\"word-report\"")
        .nth(1)
        .and_then(|rest| rest.split('>').nth(1))
        .and_then(|text| text.split('<').next())
        .unwrap_or_default()
        .to_string()
}

/// Settings on Words, on the settings screen.
fn on_words() -> (Ui, std::rc::Rc<crate::audio::fake::Recorder>) {
    let (mut ui, recorder) = Ui::app_with_settings(test_settings());
    ui.click("nav-settings");
    ui.click("seg-words");
    (ui, recorder)
}

/// Words starts on the starter list, and says so; the level, the unlock
/// order and the group size go, as a word has none of them.
#[test]
fn words_start_on_the_starter_list_and_drop_what_a_word_has_none_of() {
    run(|| async {
        let (ui, _recorder) = on_words();
        assert!(
            ui.has("Starter words, sent whole · 81 words"),
            "{}",
            ui.html()
        );
        assert!(ui.has("Until you add your own, Words sends a starter list"));
        // How many words there are to draw from is said; the list itself is
        // folded away, and no character is shown on its own: a word is sent
        // whole.
        assert!(ui.has("81 words to draw from"), "{}", ui.html());
        assert!(!ui.has(">QTH<"), "the list starts folded");
        assert!(!ui.has("ch sent-row"), "single characters in Words mode");
        assert_eq!(value_of(&ui, "field-word-list"), "");
        for gone in [
            "field-level",
            "pill-lcwo",
            "from-group-size",
            "fixed-group-size",
        ] {
            assert!(!ui.shows(gone), "{gone} has no meaning for a word");
        }
        assert!(ui.shows("number-words-per-session"));
        assert!(!ui.shows("btn-starter-words"), "nothing to go back from");
    });
}

/// The starter list unfolds to be looked through, and folds away again.
#[test]
fn the_starter_list_unfolds_and_folds() {
    run(|| async {
        let (mut ui, _recorder) = on_words();
        let toggle = "disclosure-starter-list";
        assert!(ui.input_tag(toggle).contains("aria-expanded=\"false\""));
        ui.click(toggle);
        assert!(ui.input_tag(toggle).contains("link-toggle open"));
        assert!(ui.has(">QTH<") && ui.has(">73<"), "{}", ui.html());
        ui.click(toggle);
        assert!(!ui.has(">QTH<"));
    });
}

/// Callsigns are sent whole too: a few calls of the tier, with more to
/// come, and no character on its own.
#[test]
fn callsigns_show_calls_not_characters() {
    run(|| async {
        let (mut ui, _recorder) = Ui::app();
        ui.click("nav-settings");
        ui.click("seg-callsigns");
        assert!(ui.has("What this tier sends"), "{}", ui.html());
        assert_eq!(ui.html().matches("class=\"call\"").count(), 4);
        assert!(ui.has(">…<"), "{}", ui.html());
        assert!(!ui.has("ch sent-row"), "single characters in Callsign mode");
        // Character groups are drawn a character at a time, and show them.
        ui.click("seg-koch");
        assert!(ui.has("ch sent-row"));
    });
}

/// Words typed or pasted in are counted as they are typed, and taken on
/// leaving the box: upper-cased, each once, what cannot be sent left out
/// and said.
#[test]
fn pasted_words_are_counted_as_typed_and_taken_on_leaving_the_box() {
    run(|| async {
        let (mut ui, _recorder) = on_words();
        let pasted = "paris\ncodex\n73\nCODEX\nmünchen\n";
        let _ = ui.take_work();
        ui.type_into("field-word-list", pasted);
        assert_eq!(
            word_report(&ui),
            "3 words · 1 left out: a character Morse has no code for, or longer than 20 · 1 repeated — taken when you leave the box"
        );
        assert!(
            !ui.input_tag("field-word-list").contains("value=\"paris"),
            "a key redrew the box"
        );
        ui.commit("field-word-list", pasted);
        assert_eq!(
            word_report(&ui),
            "3 words · 1 left out: a character Morse has no code for, or longer than 20 · 1 repeated"
        );
        assert_eq!(value_of(&ui, "field-word-list"), "PARIS\nCODEX\n73");
        assert!(ui.has("Your own words, sent whole · 3 words"));
        let stored = ui.stored_settings().await;
        assert_eq!(stored.curriculum.word_list, ["PARIS", "CODEX", "73"]);

        // Emptying the box goes back to the starter words.
        ui.commit("field-word-list", "  \n");
        assert_eq!(word_report(&ui), "Back to the starter words.");
        assert!(ui.has("Starter words, sent whole · 81 words"));
        assert!(ui.stored_settings().await.curriculum.word_list.is_empty());
    });
}

/// A text file loads as the words, and says what it came to; Starter words
/// puts the starter list back.
#[test]
fn a_text_file_loads_as_the_words() {
    run(|| async {
        let (mut ui, _recorder) = on_words();
        let text = "cq\r\nde\r\ntest\r\n";
        ui.choose_file("file-word-list", "contest.txt", text, text.len() as u64);
        ui.advance(20).await;
        assert_eq!(word_report(&ui), "contest.txt: 3 words");
        assert_eq!(value_of(&ui, "field-word-list"), "CQ\nDE\nTEST");
        assert_eq!(
            ui.stored_settings().await.curriculum.word_list,
            ["CQ", "DE", "TEST"]
        );

        ui.click("btn-starter-words");
        assert_eq!(word_report(&ui), "Back to the starter words.");
        assert!(ui.stored_settings().await.curriculum.word_list.is_empty());
    });
}

/// A file with nothing in it that can be sent, or far too large to be a
/// word list, leaves the words as they were, and says why.
#[test]
fn a_file_that_is_no_word_list_leaves_the_words_as_they_were() {
    run(|| async {
        let (mut ui, _recorder) = on_words();
        ui.commit("field-word-list", "PARIS");
        ui.choose_file("file-word-list", "umlauts.txt", "äöü ñé", 12);
        ui.advance(20).await;
        assert_eq!(
            word_report(&ui),
            "umlauts.txt: no word in it can be sent, so the words are as they were."
        );
        ui.choose_file("file-word-list", "film.mkv", "CQ", 50_000_000);
        ui.advance(20).await;
        assert_eq!(
            word_report(&ui),
            "film.mkv is over a megabyte, far more than a word list: nothing was read."
        );
        assert_eq!(ui.stored_settings().await.curriculum.word_list, ["PARIS"]);

        // Typed text that keeps nothing goes, and the box shows the words.
        ui.type_into("field-word-list", "äöü");
        ui.commit("field-word-list", "äöü");
        assert_eq!(
            word_report(&ui),
            "The list: no word in it can be sent, so the words are as they were."
        );
        assert_eq!(value_of(&ui, "field-word-list"), "PARIS");
    });
}

/// A session on Words sends the words of the list, whole, and the practice
/// screen counts words rather than groups of a size.
#[test]
fn a_session_on_words_sends_the_words_of_the_list() {
    run(|| async {
        let (mut ui, recorder) = on_words();
        ui.commit("field-word-list", "PARIS CODEX QRL?");
        ui.click("nav-practice");
        assert!(ui.has("Your words · 3"), "{}", ui.html());
        assert!(ui.has(">Words<"));
        assert!(!ui.has("% to pass"), "a word list has no level run to show");
        assert!(!ui.has("moves the level for you"));
        ui.click("btn-start-training");
        assert!(ui.run_until(5_000, |ui| ui.has("Your turn")).await);
        let sent = recorder.texts().first().cloned().expect("a word was sent");
        assert!(
            ["PARIS", "CODEX", "QRL?"].contains(&sent.as_str()),
            "{sent}"
        );
        let stored = ui.store.load_settings();
        assert_eq!(stored.curriculum.char_set_mode, CharSetMode::Words);
    });
}

/// The contest card's presets each set how fast the callers come, as the
/// card and the Speed card show it, and nothing else: the tier, the run
/// style, the callers' habits, the band and your own speed stay as they
/// were. The card reads Custom once the callers' speed is changed, and a
/// change to anything else leaves the preset chosen. The speed is the
/// Speed card's, which group training uses too, and the card says so.
#[test]
fn a_contest_preset_sets_the_callers_speed_and_nothing_else() {
    run(|| async {
        let custom = crate::ui::settings_charset::PRESET_CUSTOM;
        assert!(custom.contains("which group training uses too"));
        // As the page escapes it.
        let custom = &custom.replace('\'', "&#39;");
        let (mut ui, _recorder) = Ui::app_with_settings(crate::testing::contest_settings());
        let before = ui.store.load_settings();
        ui.click("nav-settings");
        assert!(ui.has(custom));
        ui.click("preset-first-qso");
        assert!(ui.input_tag("preset-first-qso").contains("active"));
        assert!(
            ui.has("Callers\u{a0}at\u{a0}15–18\u{a0}WPM"),
            "{}",
            ui.html()
        );
        assert_eq!(value_of(&ui, "from-character-speed"), "15");
        assert_eq!(value_of(&ui, "to-character-speed"), "18");
        let stored = ui.stored_settings().await;
        assert_eq!(
            cw_core::ContestPreset::of(&stored),
            Some(cw_core::ContestPreset::FirstQso)
        );
        assert_eq!(stored.curriculum, before.curriculum, "the tier");
        assert_eq!(stored.contest, before.contest, "the contest's own");
        assert_eq!(stored.band, before.band, "the band");

        ui.click("preset-contester");
        assert!(ui.has("Callers\u{a0}at\u{a0}28–36\u{a0}WPM"));
        // The callers' speed moved by hand: the settings are your own.
        ui.commit("from-character-speed", "30");
        ui.advance(50).await;
        assert!(ui.has(custom));
        assert!(!ui.input_tag("preset-contester").contains("active"));
        // Back on Club, then the tier, the run style and the habits picked
        // by hand: Club still, as a preset sets none of them.
        ui.click("preset-club");
        assert!(ui.has("Callers\u{a0}at\u{a0}20–26\u{a0}WPM"));
        ui.commit("field-level", "5");
        ui.click("style-pile-up");
        ui.click("habits-rough");
        ui.advance(50).await;
        assert!(ui.input_tag("preset-club").contains("active"));
        assert!(!ui.has(custom));
    });
}

/// In Contest mode the Band card says how the callers operate, a sentence
/// for each habit that follows the contest, and keeps the one picked; in
/// group training it has no callers to say it of.
#[test]
fn the_band_card_sets_the_callers_habits_in_a_contest() {
    run(|| async {
        let (mut ui, _recorder) = Ui::app_with_settings(crate::testing::contest_settings());
        ui.click("nav-settings");
        assert!(ui.has("Caller habits"));
        assert!(
            ui.has("Stations, callers"),
            "the card note names the habits"
        );
        assert!(ui.input_tag("habits-easy").contains("active"));
        for said in [
            "Easy: plain digits, nobody slips, and callers slow down for you",
            "Real: numbers as contesters send them, T\u{a0}for\u{a0}0, N\u{a0}for\u{a0}9 (TT7\u{a0}for\u{a0}7), now and then O\u{a0}for\u{a0}0 and ENN\u{a0}for\u{a0}5NN; nobody slows down for you.",
            "Rough: real, with careless callers who fumble a serial, send their exchange too soon",
        ] {
            assert!(ui.has(said), "{said}");
        }
        // The examples follow the contest: a zone in CQ WW, and nothing to
        // cut nor a serial to fumble in Field Day.
        ui.click("contest-cq-ww");
        assert!(ui.has("(A4\u{a0}for\u{a0}14)"));
        assert!(ui.has("careless callers who send their exchange too soon"));
        ui.click("contest-field-day");
        assert!(ui.has("Real: Field Day callers send no number cut, so it plays as Easy"));
        ui.click("contest-cq-wpx");
        assert!(!ui.has("and, with Rough habits, now and then a station"));
        ui.click("habits-rough");
        ui.advance(50).await;
        assert!(ui.input_tag("habits-rough").contains("active"));
        assert_eq!(
            ui.stored_settings().await.contest.contest_habits,
            cw_core::CallerHabits::Rough
        );
        // At one at a time the band has QRM on it all the same.
        assert!(ui.has("and, with Rough habits, now and then a station that is not calling you"));
        // And so does a pile-up of one caller at a time.
        ui.click("style-pile-up");
        assert!(ui.has(
            "only one caller at a time, and with Rough habits now and then a station on your frequency that is not calling you"
        ));
        ui.click("style-single-caller");
        // Its reset puts them back with the rest of the band.
        ui.click("reset-band-conditions");
        ui.advance(50).await;
        assert_eq!(
            ui.stored_settings().await.contest.contest_habits,
            cw_core::CallerHabits::Easy
        );
        // Callsigns is group training: no callers, no habits.
        ui.click("seg-callsigns");
        ui.advance(50).await;
        assert!(!ui.has("Caller habits"));
        assert!(!ui.shows("habits-easy"));
    });
}

/// Flutter is on the Band card in Contest mode only, after the fading,
/// off until it is turned on, says what it does, is stored, no preset
/// touches it, and it goes back off with its card's reset.
#[test]
fn flutter_is_a_band_switch_in_contest_mode() {
    run(|| async {
        let (mut ui, _recorder) = Ui::app_with_settings(test_settings());
        ui.click("nav-settings");
        assert!(!ui.shows("switch-flutter"), "not outside Contest");

        let (mut ui, _recorder) = Ui::app_with_settings(crate::testing::contest_settings());
        ui.click("nav-settings");
        assert!(ui.shows("switch-flutter"));
        assert!(!ui.input_tag("switch-flutter").contains("checked"));
        assert!(ui.has(
            "Three callers in ten flutter, their signal rising and falling 3 to 33 times a second"
        ));
        assert!(ui.has("Stations, callers&#39; habits, fading and flutter"));
        // It sits on the Band card, after the fading.
        let html = ui.html();
        let fading = html.find("QSB fading").expect("the fading");
        let switch = html.find(r#"id="switch-flutter""#).expect("the switch");
        let statics = html.find("QRN static").expect("the static");
        assert!(fading < switch && switch < statics);

        ui.commit("switch-flutter", "true");
        assert!(ui.stored_settings().await.contest.contest_flutter);
        ui.click("preset-club");
        ui.click("preset-first-qso");
        assert!(
            ui.stored_settings().await.contest.contest_flutter,
            "a preset leaves it alone"
        );
        ui.click("reset-band-conditions");
        assert!(!ui.stored_settings().await.contest.contest_flutter);
    });
}

/// Full break-in is on the Tone and volume card in Contest mode only,
/// after your sending, off as MorseRunner ships it, says what each way
/// sounds like, is stored, no preset touches it, and the card's reset turns
/// it back off.
#[test]
fn full_break_in_is_a_tone_card_switch_in_contest_mode() {
    run(|| async {
        let (mut ui, _recorder) = Ui::app_with_settings(test_settings());
        ui.click("nav-settings");
        assert!(!ui.shows("switch-full-break-in-qsk"), "not outside Contest");

        let (mut ui, _recorder) = Ui::app_with_settings(crate::testing::contest_settings());
        ui.click("nav-settings");
        assert!(ui.shows("switch-full-break-in-qsk"));
        assert!(!ui.input_tag("switch-full-break-in-qsk").contains("checked"));
        assert!(ui.has("Off, the receiver is muted while you send, as on a rig without it"));
        let html = ui.html();
        let sending = html
            .find(r#"id="slider-your-sending""#)
            .expect("your sending");
        let switch = html
            .find(r#"id="switch-full-break-in-qsk""#)
            .expect("the switch");
        assert!(sending < switch);

        ui.commit("switch-full-break-in-qsk", "true");
        assert!(ui.stored_settings().await.contest.contest_qsk);
        ui.click("preset-first-qso");
        ui.click("preset-contester");
        assert!(
            ui.stored_settings().await.contest.contest_qsk,
            "a preset leaves it alone"
        );
        ui.click("reset-tone-volume");
        assert!(!ui.stored_settings().await.contest.contest_qsk);
    });
}

/// Show copy is on Session shape in Contest mode only, says what it shows
/// and what it costs a run, is stored, goes back off with its card's reset,
/// and no preset touches it.
#[test]
fn show_copy_is_a_session_shape_switch_in_contest_mode() {
    run(|| async {
        let (mut ui, _recorder) = Ui::app_with_settings(test_settings());
        ui.click("nav-settings");
        assert!(!ui.shows("switch-show-copy"), "not outside Contest");

        let (mut ui, _recorder) = Ui::app_with_settings(crate::testing::contest_settings());
        ui.click("nav-settings");
        assert!(ui.shows("switch-show-copy"));
        assert!(
            ui.has("Shows each caller&#39;s text on the run screen a moment after it is sent.")
        );
        assert!(ui.has("marked assisted: it counts in every stat, but is greyed on the board and never placed there"));
        // It sits on Session shape, after the minutes a run lasts.
        let html = ui.html();
        let shape = html.find(">Session shape<").expect("the card");
        let minutes = html.find("Minutes per run").expect("its minutes");
        let switch = html.find(r#"id="switch-show-copy""#).expect("the switch");
        assert!(shape < minutes && minutes < switch);

        ui.commit("switch-show-copy", "true");
        assert!(ui.stored_settings().await.contest.contest_show_copy);
        ui.click("preset-contester");
        assert!(
            ui.stored_settings().await.contest.contest_show_copy,
            "a preset leaves it alone"
        );
        ui.click("reset-session-shape");
        assert!(!ui.stored_settings().await.contest.contest_show_copy);
    });
}

/// Your speed is on the Speed card in Contest mode only: until you set
/// one, the middle of the callers' range, which it says; set, it is yours,
/// stored, with a press back to the middle; the card's reset puts it back
/// to the middle too, and a preset leaves it alone.
#[test]
fn your_speed_is_on_the_speed_card_in_contest_mode() {
    run(|| async {
        let (mut ui, _recorder) = Ui::app_with_settings(test_settings());
        ui.click("nav-settings");
        assert!(!ui.shows("number-your-speed"), "not outside Contest");

        let (mut ui, _recorder) = Ui::app_with_settings(crate::testing::contest_settings());
        ui.click("nav-settings");
        assert_eq!(value_of(&ui, "number-your-speed"), "30");
        assert!(
            ui.has("The middle of the callers&#39; range until you set it. The run screen&#39;s − and + move it"),
            "{}",
            ui.html()
        );
        assert!(!ui.shows("btn-callers-middle"));
        assert!(ui.has(
            "Callers come at speeds drawn from these ranges. Group training sends at them too."
        ));
        // It sits on the Speed card, after the callers' ranges.
        let html = ui.html();
        let card = html.find(">Speed<").expect("the card");
        let range = html.find("Character speed").expect("the callers' range");
        let yours = html.find(r#"id="number-your-speed""#).expect("your speed");
        assert!(card < range && range < yours);

        ui.commit("number-your-speed", "34");
        assert_eq!(ui.stored_settings().await.contest.my_wpm, 34);
        assert_eq!(value_of(&ui, "number-your-speed"), "34");
        assert!(ui.has("Yours from run to run"));
        ui.click("btn-callers-middle");
        assert_eq!(ui.stored_settings().await.contest.my_wpm, 0);
        assert!(!ui.shows("btn-callers-middle"));

        ui.click("more-number-your-speed");
        assert_eq!(ui.stored_settings().await.contest.my_wpm, 31);
        ui.click("preset-club");
        assert_eq!(
            ui.stored_settings().await.contest.my_wpm,
            31,
            "a preset leaves it alone"
        );
        assert!(ui.input_tag("preset-club").contains("active"));
        // Back to the middle, it follows the callers' range a preset sets.
        ui.click("btn-callers-middle");
        assert_eq!(value_of(&ui, "number-your-speed"), "23");
        // A middle on a half reads as the run bar reads it: 16.5 as 17.
        ui.click("preset-first-qso");
        assert_eq!(value_of(&ui, "number-your-speed"), "17");
        ui.commit("number-your-speed", "40");
        ui.advance(50).await;
        assert!(
            ui.input_tag("preset-first-qso").contains("active"),
            "your speed is not the preset's"
        );
        ui.click("reset-speed");
        assert_eq!(ui.stored_settings().await.contest.my_wpm, 0, "the reset");
    });
}

/// The callers' adaptive speed is on the Speed card in Contest mode only,
/// where group training's switch sits and in its place, on as it ships,
/// with a sentence saying what it does. On, the aim it shares with group
/// training shows beside it. It is stored, group training's switch is left
/// as it was, a preset leaves it alone, and the card's reset turns it back
/// on. One speed leaves it nothing to adapt, which it says.
#[test]
fn the_callers_adaptive_speed_is_on_the_speed_card_in_contest_mode() {
    run(|| async {
        let (mut ui, _recorder) = Ui::app_with_settings(test_settings());
        ui.click("nav-settings");
        assert!(
            !ui.shows("switch-adapt-callers-speed-to-me"),
            "not outside Contest"
        );
        assert!(ui.shows("switch-adapt-speed-to-me"));

        let mut settings = crate::testing::contest_settings();
        settings.playback.char_wpm_min = 20.0;
        settings.playback.effective_wpm_min = 20.0;
        let (mut ui, _recorder) = Ui::app_with_settings(settings);
        ui.click("nav-settings");
        assert!(!ui.shows("switch-adapt-speed-to-me"), "group training's");
        let switch = "switch-adapt-callers-speed-to-me";
        assert!(ui.input_tag(switch).contains("checked"), "on as it ships");
        assert!(ui.has(
            "New callers come faster while you copy calls and numbers clean, slower when you miss them; those already calling keep their speed."
        ));
        assert!(ui.has("Balanced aims for 94% of characters right."));
        // On the Speed card, after the callers' ranges and before your speed.
        let html = ui.html();
        let range = html.find("Character speed").expect("the callers' range");
        let adapt = html.find(r#"id="switch-adapt-callers-speed-to-me""#);
        let yours = html.find(r#"id="number-your-speed""#).expect("your speed");
        assert!(adapt.is_some_and(|adapt| range < adapt && adapt < yours));

        ui.commit(switch, "false");
        let stored = ui.stored_settings().await;
        assert!(!stored.contest.contest_adapt_speed);
        assert!(
            !stored.playback.adaptive_speed,
            "group training's is its own"
        );
        assert!(!ui.has("Balanced aims for 94% of characters right."));
        ui.click("preset-club");
        assert!(
            !ui.stored_settings().await.contest.contest_adapt_speed,
            "a preset"
        );
        ui.click("reset-speed");
        assert!(
            ui.stored_settings().await.contest.contest_adapt_speed,
            "the reset"
        );

        // Its reset turns it back on, and the aim it brings onto the card
        // goes back too.
        ui.click("seg-stretch");
        ui.commit(switch, "false");
        ui.click("reset-speed");
        let stored = ui.stored_settings().await;
        assert!(stored.contest.contest_adapt_speed);
        assert_eq!(stored.curriculum.aim, Aim::Balanced, "the aim");

        // One speed, tied: only Character speed shows, and only it is named.
        let (mut fixed, _recorder) = Ui::app_with_settings(crate::testing::contest_settings());
        fixed.click("nav-settings");
        assert!(
            fixed.has(
                "Give Character speed a range to use this: one speed leaves nothing to adapt."
            )
        );
        assert!(!fixed.has("Balanced aims for 94% of characters right."));
        fixed.commit("switch-tie-effective-speed-to-character-speed", "false");
        assert!(fixed.has(
            "Give Character speed or Effective speed a range to use this: one speed leaves nothing to adapt."
        ));
    });
}

/// An effective speed set above the fastest character speed is sent at
/// that, as the spacing is never faster than the characters. The card says
/// what it is used as under its range, and the adapt switch why a range
/// there leaves one speed, rather than asking for a range it has.
#[test]
fn an_effective_speed_above_the_characters_says_what_it_is_used_as() {
    run(|| async {
        let mut settings = test_settings();
        settings.playback.char_wpm_min = 20.0;
        settings.playback.char_wpm_max = 20.0;
        settings.playback.link_char_wpm = true;
        settings.playback.link_char_to_effective = false;
        settings.playback.effective_wpm_min = 25.0;
        settings.playback.effective_wpm_max = 35.0;
        let (mut ui, _recorder) = Ui::app_with_settings(settings);
        ui.click("nav-settings");
        assert!(ui.has("25–35 WPM"), "the range as set");
        let held = |ui: &Ui| {
            ui.html()
                .split("id=\"effective-held\"")
                .nth(1)
                .map(str::to_string)
        };
        assert!(
            held(&ui).is_some_and(|line| line.contains(
                ">Used as 20\u{a0}WPM: the spacing is never faster than Character speed.<"
            )),
            "{}",
            ui.html()
        );
        assert!(ui.has(
            "Effective speed is never faster than Character speed, so it is sent at 20\u{a0}WPM and one speed is left: give Character speed a range, or Effective speed one under 20\u{a0}WPM, to use this."
        ));
        assert!(!ui.has("Give Character speed or Effective speed a range"));

        // Part of it under the characters is a range to adapt in, used up
        // to them.
        ui.commit("from-effective-speed", "12");
        assert!(held(&ui).is_some_and(|line| line.contains(">Used as 12–20\u{a0}WPM")));
        assert!(ui.has("Within these speeds: faster while you copy the characters"));

        // All of it under them is heard as set, and nothing is said.
        ui.commit("to-effective-speed", "18");
        assert!(!ui.shows("effective-held"));
    });
}

/// The word space is a multiple of the standard one, which shortens it
/// as well as lengthens it, and an answer timeout of 0 never moves on:
/// each says so at its control and in the card's help.
#[test]
fn the_word_space_and_a_timeout_of_zero_say_what_they_do() {
    run(|| async {
        let (mut ui, _recorder) = Ui::app_with_settings(test_settings());
        ui.click("nav-settings");
        assert!(ui.shows("number-word-space"));
        assert!(ui.has("1× is standard, 2× twice as long, 0.5× half."));
        assert!(ui.has("; at 0 it never does."));
        ui.click("help-speed");
        assert!(ui.has("more lengthens it, less shortens it, down to half"));
        ui.click("help-session-shape");
        assert!(ui.has("At 0 it never moves on by itself"));
    });
}

/// A card's reset can be undone from the message it leaves: Your station's
/// puts back the call that took typing, and a change made on another card
/// in between stays. In Contest mode the Character set's reset puts the
/// contest's own choices back and stays in Contest.
#[test]
fn a_cards_reset_can_be_undone() {
    run(|| async {
        let (mut ui, _recorder) = Ui::app_with_settings(crate::testing::contest_settings());
        ui.click("nav-settings");
        ui.click("reset-your-station");
        assert!(ui.has("Your station reset to defaults"), "{}", ui.html());
        assert!(ui.shows("toast-undo"));
        assert_eq!(ui.stored_settings().await.contest.my_call, "");
        ui.commit("number-minutes-per-run", "12");
        ui.click("toast-undo");
        assert!(!ui.shows("toast-undo"));
        let stored = ui.stored_settings().await;
        assert_eq!(stored.contest.my_call, "DL1ABC");
        assert_eq!(
            stored.contest.contest_minutes, 12,
            "the other card's change"
        );

        ui.click("contest-cq-ww");
        ui.click("reset-character-set");
        assert!(ui.input_tag("seg-contest").contains("active"));
        assert!(ui.input_tag("contest-cq-wpx").contains("active"));
        let stored = ui.stored_settings().await;
        assert_eq!(stored.curriculum.char_set_mode, CharSetMode::Contest);
        ui.click("toast-undo");
        assert!(ui.input_tag("contest-cq-ww").contains("active"));

        // A card already at its defaults has nothing to undo.
        ui.click("reset-keying-envelope");
        assert!(!ui.shows("toast-undo"));
    });
}

/// The Character set's reset leaves an unlock order of the learner's own as
/// it is, as it does the word list: what took typing is theirs.
#[test]
fn the_character_sets_reset_leaves_an_order_of_ones_own() {
    run(|| async {
        let mut settings = test_settings();
        settings.curriculum.word_list = vec!["PARIS".to_string()];
        let (mut ui, _recorder) = Ui::app_with_settings(settings);
        ui.click("nav-settings");
        ui.commit("field-sequence-order", "KMURESN");
        ui.click("reset-character-set");
        assert!(ui.input_tag("pill-your-own").contains("active"));
        assert_eq!(value_of(&ui, "field-sequence-order"), "KMURESN");
        let stored = ui.stored_settings().await;
        assert_eq!(stored.curriculum.char_set_mode, CharSetMode::Mixed);
        assert_eq!(stored.curriculum.word_list, ["PARIS"]);
    });
}

/// Your sending is on the Tone and volume card in Contest mode only, in
/// decibels from the loudest down to off, at -3 dB until you set it; it
/// is stored, a preset leaves it alone, and the card's reset puts it back.
#[test]
fn your_sending_is_on_the_tone_card_in_contest_mode() {
    run(|| async {
        let (mut ui, _recorder) = Ui::app_with_settings(test_settings());
        ui.click("nav-settings");
        assert!(!ui.shows("slider-your-sending"), "not outside Contest");

        let (mut ui, _recorder) = Ui::app_with_settings(crate::testing::contest_settings());
        ui.click("nav-settings");
        assert!(ui.shows("slider-your-sending"));
        assert!(
            ui.has(r#"<span class="slider-value">−3 dB</span>"#),
            "{}",
            ui.html()
        );
        // Apart from the station strength, and level with an average caller
        // where it starts, as cw-core keeps that.
        assert!(ui.has(
            "Your own messages in a run, apart from the station strength above: 0 dB is the loudest, −3 dB about level with an average caller, and Off silent"
        ));
        // It sits on Tone and volume, after the station strength.
        let html = ui.html();
        let card = html
            .find(">Tone &amp; volume<")
            .or_else(|| html.find(">Tone &#38; volume<"))
            .expect("the card");
        let strength = html
            .find(r#"id="link-station-strength""#)
            .expect("the station strength");
        let sending = html
            .find(r#"id="slider-your-sending""#)
            .expect("your sending");
        assert!(card < strength && strength < sending);

        ui.click("less-slider-your-sending");
        assert_eq!(ui.stored_settings().await.contest.my_sending_db, -6);
        assert!(ui.has(r#"<span class="slider-value">−6 dB</span>"#));
        ui.type_into("slider-your-sending", "-60");
        assert_eq!(ui.stored_settings().await.contest.my_sending_db, -60);
        assert!(ui.has(r#"<span class="slider-value">Off</span>"#));
        assert!(
            ui.input_tag("less-slider-your-sending")
                .contains("disabled")
        );
        ui.type_into("slider-your-sending", "0");
        assert!(ui.has(r#"<span class="slider-value">0 dB</span>"#));
        ui.click("preset-contester");
        assert_eq!(
            ui.stored_settings().await.contest.my_sending_db,
            0,
            "a preset leaves it alone"
        );
        ui.click("reset-tone-volume");
        assert_eq!(ui.stored_settings().await.contest.my_sending_db, -3);
    });
}

/// Volume is the first thing on the Tone and volume card, in every mode:
/// at full until you set it, down a decibel a step to 26 down, stored, and
/// put back by the card's reset. The range under it is the station's strength,
/// in decibels, and says it is not how loud anything plays.
#[test]
fn the_volume_heads_the_tone_card_and_the_range_is_station_strength() {
    run(|| async {
        let (mut ui, _recorder) = Ui::app_with_settings(test_settings());
        ui.click("nav-settings");
        assert!(ui.has(r#"<span class="slider-value">Full</span>"#));
        assert!(ui.input_tag("more-slider-volume").contains("disabled"));
        let html = ui.html();
        let volume = html.find(r#"id="slider-volume""#).expect("the volume");
        let tone = html.find(r#"id="link-side-tone""#).expect("the side tone");
        let strength = html
            .find(r#"id="link-station-strength""#)
            .expect("the station strength");
        assert!(volume < tone && tone < strength);
        assert!(ui.has(
            "noise, not how loud it plays: 0 dB is the strongest. A range draws a fresh strength per group."
        ));
        assert!(
            !ui.has(r#"<span class="field-label">Volume</span>"#),
            "no range is called volume"
        );

        // A decibel a step, the share kept.
        ui.click("less-slider-volume");
        assert_eq!(ui.stored_settings().await.band.master_volume, 0.8913);
        assert!(ui.has(r#"<span class="slider-value">−1 dB</span>"#));
        ui.type_into("slider-volume", "-6");
        assert_eq!(ui.stored_settings().await.band.master_volume, 0.5012);
        assert!(ui.has(r#"<span class="slider-value">−6 dB</span>"#));
        ui.type_into("slider-volume", "-26");
        assert_eq!(ui.stored_settings().await.band.master_volume, 0.0501);
        assert!(ui.has(r#"<span class="slider-value">−26 dB</span>"#));
        assert!(ui.input_tag("less-slider-volume").contains("disabled"));
        ui.click("reset-tone-volume");
        assert_eq!(ui.stored_settings().await.band.master_volume, 1.0);
        assert!(ui.has(r#"<span class="slider-value">Full</span>"#));
    });
}

/// Every step of the volume slider is a decibel, from full to the bottom,
/// the share it keeps reading back to the step it was set at; a share saved
/// before it was in decibels reads at the step nearest it.
#[test]
fn every_step_of_the_volume_is_a_decibel() {
    use super::widgets::{db_share, share_db};
    let floor = cw_core::MASTER_VOLUME_MIN;
    let volume_db = |share: f64| share_db(share, floor);
    let volume_share = |db: f64| db_share(db, floor);
    let bottom = volume_db(floor);
    assert_eq!(bottom, -26.0);
    let mut step = 0.0;
    let mut louder = volume_share(0.0);
    assert_eq!(louder, 1.0);
    while step > bottom {
        step -= 1.0;
        let share = volume_share(step);
        assert_eq!(volume_db(share), step, "{share} at {step} dB");
        let apart = 20.0 * (louder / share).log10();
        assert!((apart - 1.0).abs() < 0.01, "{apart:.3} dB below {step} dB");
        louder = share;
    }
    assert!(louder >= cw_core::MASTER_VOLUME_MIN);
    assert_eq!(volume_db(0.5), -6.0);
    assert_eq!(volume_db(0.05), bottom);
}

/// The station strength is shown and typed in decibels down from the
/// strongest, and stored as the share it always was: a share saved before
/// reads at the decibel nearest it, and the bottom of the range is the
/// weakest share.
#[test]
fn the_station_strength_is_typed_in_decibels() {
    run(|| async {
        let mut settings = test_settings();
        settings.band.link_volume = false;
        settings.band.volume_min = 0.7;
        settings.band.volume_max = 1.0;
        let (mut ui, _recorder) = Ui::app_with_settings(settings);
        ui.click("nav-settings");
        assert!(ui.has("\u{2212}3 to 0 dB"), "{}", ui.html());
        ui.type_into("from-station-strength", "-12");
        ui.commit("from-station-strength", "-12");
        ui.advance(50).await;
        assert_eq!(ui.stored_settings().await.band.volume_min, 0.2512);
        assert!(ui.has("\u{2212}12 to 0 dB"));
        ui.type_into("from-station-strength", "-40");
        ui.commit("from-station-strength", "-40");
        ui.advance(50).await;
        assert_eq!(
            ui.stored_settings().await.band.volume_min,
            cw_core::VOLUME_MIN
        );
        assert!(ui.has("\u{2212}20 to 0 dB"));
    });
}

/// Your call in TU is on the contest card, with one caller at a time or a
/// pile-up, where the contest's TU leaves it to you: every QSO (as it
/// starts), every third or never, each saying what it does, and the card's
/// example QSO, your first, signing its TU as that QSO would. Not for the
/// Sprint, whose TU always signs, or Simple QSO, whose TU is its own, which
/// keep what was chosen; the card's reset puts every QSO back.
#[test]
fn your_call_in_tu_is_on_the_contest_card() {
    run(|| async {
        let (mut ui, _recorder) = Ui::app_with_settings(crate::testing::contest_settings());
        ui.click("nav-settings");
        // A single caller's TU signs as a pile-up's does.
        assert!(ui.input_tag("tu-every-qso").contains("active"));
        assert!(ui.has("Your call in TU"));
        assert!(ui.has("Every TU signs with your call"));
        // The example QSO is your first: every QSO's TU signs it.
        assert!(ui.has(">TU DL1ABC<"), "the example's TU signs");
        ui.click("tu-never");
        assert_eq!(
            ui.stored_settings().await.contest.contest_tu_call,
            cw_core::TuCall::Never
        );
        assert!(ui.has("No TU signs with your call; the callers still call after each"));
        assert!(ui.has(">TU<") && !ui.has(">TU DL1ABC<"), "the example's TU");
        ui.click("style-pile-up");
        assert!(ui.input_tag("tu-never").contains("active"));
        ui.click("tu-every-third");
        assert!(ui.has(
            "Every third QSO&#39;s TU signs with your call, so a station tuning in hears who is running"
        ));
        assert!(!ui.has(">TU DL1ABC<"), "your first QSO is not your third");
        for (contest, shown) in [
            ("contest-na-sprint", false),
            ("contest-simple-qso", false),
            ("contest-field-day", true),
            ("contest-cq-ww", true),
            ("contest-arrl-dx", true),
            ("contest-cq-wpx", true),
        ] {
            ui.click(contest);
            assert_eq!(ui.shows("tu-every-qso"), shown, "{contest}");
            assert_eq!(
                ui.stored_settings().await.contest.contest_tu_call,
                cw_core::TuCall::Third,
                "{contest}: kept while it is not shown"
            );
        }
        ui.click("reset-character-set");
        assert_eq!(
            ui.stored_settings().await.contest.contest_tu_call,
            cw_core::TuCall::Every
        );
    });
}

/// Every card on the settings screen has an ⓘ that opens what its options
/// do, out of the way until asked for: in group training, and in Contest
/// mode with the station's card.
#[test]
fn every_settings_card_says_what_its_options_do() {
    run(|| async {
        let (mut ui, _recorder) = Ui::app_with_settings(test_settings());
        ui.click("nav-settings");
        for (card, says) in [
            ("experience", "A starting point for where you are"),
            (
                "character-set",
                "unlocked one at a time in the order chosen",
            ),
            ("session-shape", "How many groups one session sends"),
            ("speed", "Farnsworth: the characters stay fast"),
            ("tone-volume", "The pitch you hear the Morse at"),
            ("keying-envelope", "How each dit and dah starts and stops"),
            (
                "auto-level",
                "How many good sessions it takes to unlock a new character",
            ),
            (
                "character-sampling",
                "How much more often the characters you miss are sent",
            ),
            ("recognition", "How fast counts as what on Letter accuracy"),
            (
                "band-conditions",
                "Slow swells and dips of each station, every one on a path of its own",
            ),
        ] {
            let help = format!("help-{card}");
            assert!(!ui.has(says), "{card} keeps its help closed");
            ui.click(&help);
            assert!(ui.has(says), "{card} opens its help: {}", ui.html());
            ui.click(&help);
            assert!(!ui.has(says), "{card} closes it again");
        }

        let mut contest = test_settings();
        contest.curriculum.char_set_mode = CharSetMode::Contest;
        let (mut ui, _recorder) = Ui::app_with_settings(contest);
        ui.click("nav-settings");
        ui.click("help-your-station");
        assert!(ui.has("What you send in a run: your call"));
    });
}

/// The cards there is least call for start folded to their heads, their
/// tools hidden with what they hold, and each opens from its head and
/// folds away again; the rest of the page stays as it was.
#[test]
fn the_cards_there_is_least_call_for_start_folded_and_open_from_their_heads() {
    run(|| async {
        let (mut ui, _recorder) = Ui::app();
        ui.click("nav-settings");
        for (card, held) in [
            ("recognition", "number-fast-under"),
            ("character-sampling", "switch-thompson-sampling"),
            ("display", "btn-check-display"),
            ("privacy", "privacy-link"),
            ("credits", "credit-link"),
        ] {
            let head = format!("disclosure-{card}");
            assert!(ui.shows(&head), "{card} has no head to open it by");
            assert!(!ui.has(held), "{card} starts open");
            ui.click(&head);
            assert!(ui.has(held), "{card} does not open: {}", ui.html());
            ui.click(&head);
            assert!(!ui.has(held), "{card} does not fold away again");
        }
        // The ⓘ opens the help and leaves the card as it was, folded or
        // open.
        let help = "How fast counts as what on Letter accuracy";
        ui.click("help-recognition");
        assert!(ui.has(help), "the help opens");
        assert!(!ui.has("number-fast-under"), "the help opens the card");
        ui.click("help-recognition");
        // Anywhere else on the head opens it, as the chevron does, with its
        // reset beside the chevron, the reset folding away with what it
        // resets.
        assert!(!ui.shows("reset-recognition"));
        ui.click("head-recognition");
        assert!(
            ui.has("number-fast-under"),
            "the head does not open the card"
        );
        assert!(ui.shows("reset-recognition"));
        ui.click("help-recognition");
        assert!(ui.has("number-fast-under"), "the help folds the card");
        ui.click("head-recognition");
        assert!(
            !ui.has("number-fast-under"),
            "the head does not fold the card"
        );
        // The cards in daily use are open as they were.
        assert!(ui.shows("slider-filter-width"), "the band card is open");
    });
}

/// The titles of the cards on the page, top to bottom.
fn card_titles(ui: &Ui) -> Vec<String> {
    ui.html()
        .split("class=\"card-title\">")
        .skip(1)
        .map(|rest| {
            rest.split('<')
                .next()
                .unwrap_or_default()
                .replace("&#38;", "&")
        })
        .collect()
}

/// Settings runs from what a learner sets first to what there is least
/// call for: the cards the guided setup walks through, in the order it
/// visits them, then the band and the keying envelope, the most there is
/// to set, then the cards that start folded, so no open card sits among
/// folded ones. A contest has its station after its character set, and no
/// preset or level to move.
#[test]
fn settings_run_from_what_is_set_first_to_what_there_is_least_call_for() {
    run(|| async {
        let (mut ui, _recorder) = Ui::app();
        ui.click("nav-settings");
        assert_eq!(
            card_titles(&ui),
            [
                "Experience",
                "Character set",
                "Speed",
                "Session shape",
                "Tone & volume",
                "Auto level",
                "Band conditions",
                "Keying envelope",
                // Folded.
                "Character sampling",
                "Recognition",
                "Display",
                "Privacy",
                "Credits",
            ]
        );

        let mut contest = test_settings();
        contest.curriculum.char_set_mode = CharSetMode::Contest;
        let (mut ui, _recorder) = Ui::app_with_settings(contest);
        ui.click("nav-settings");
        assert_eq!(
            card_titles(&ui),
            [
                "Character set",
                "Your station",
                "Speed",
                "Session shape",
                "Tone & volume",
                "Band conditions",
                "Keying envelope",
                // Folded.
                "Character sampling",
                "Recognition",
                "Display",
                "Privacy",
                "Credits",
            ]
        );
    });
}
