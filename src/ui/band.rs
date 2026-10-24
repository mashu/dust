use cw_core::band::{FilterDesign, NOISE_LEVEL_PER_DB, heard_snr_db};
use cw_core::{
    BandSettings, CallerHabits, CharSetMode, ContestRules, FILTER_WIDTH_MAX, FILTER_WIDTH_MIN,
    Field, FilterShape, PILEUP_LEVEL_MAX_DB, PILEUP_LEVEL_MIN_DB, PILEUP_SPREAD_MAX,
    PILEUP_SPREAD_MIN, QSB_RATE_MAX_HZ, QSB_RATE_MIN_HZ, RangeSetting, RunStyle, STATIONS_MAX,
    STATIONS_MIN, SettingsSection, Side, TrainingSettings, filter_capacity,
};
use dioxus::prelude::*;

use crate::ui::format::counted;
use crate::ui::help;
use crate::ui::widgets::{
    CardHead, Icon, IconName, SectionReset, Seg, SettingsRange, SliderField, Switch, control_id,
};

/// The filter shapes, in the order the control offers them.
const FILTER_SHAPES: [(FilterShape, &str); 3] = [
    (FilterShape::Sharp, "Sharp"),
    (FilterShape::Soft, "Soft"),
    (FilterShape::Peaked, "Peaked"),
];

/// The callers' habits, in the order the control offers them, easiest
/// first.
const CALLER_HABITS: [(CallerHabits, &str); 3] = [
    (CallerHabits::Easy, "Easy"),
    (CallerHabits::Real, "Real"),
    (CallerHabits::Rough, "Rough"),
];

/// The word the Band card calls `habits` by.
pub(crate) fn habits_label(habits: CallerHabits) -> &'static str {
    CALLER_HABITS
        .iter()
        .find(|(each, _)| *each == habits)
        .map_or("Easy", |(_, label)| label)
}

/// What the habits line says in a contest of `rules`, from `side`: each
/// habit in a sentence, the numbers cut as that contest's callers cut
/// them, and a fumbled serial only where they send one.
fn habits_line(rules: &ContestRules, side: Side) -> String {
    let real = match rules.real_numbers(side) {
        // Each form kept whole where a narrow line breaks: `O for 0`, not
        // `O` at the end of one line and `for 0` on the next.
        Some(cuts) => format!(
            "Real: numbers as contesters send them, {}; nobody slows down for you.",
            cuts.replace(" for ", "\u{a0}for\u{a0}")
        ),
        None => format!(
            "Real: {} callers send no number cut, so it plays as Easy, but nobody slows down for you.",
            rules.kind.label()
        ),
    };
    let fumble = if rules.copied(side).contains(&Field::Nr) {
        "fumble a serial, "
    } else {
        ""
    };
    format!(
        "How the callers operate. Easy: plain digits, nobody slips, and callers slow down for you when you struggle. {real} Rough: real, with careless callers who {fumble}send their exchange too soon or take a wrong call, and now and then a station on your frequency that is not calling you."
    )
}

/// What the band card shows: the band settings, and the receiver they build.
#[derive(Clone, PartialEq)]
struct Band {
    band: BandSettings,
    /// The receiver as it is really built: the side-tone range can widen it
    /// past the slider, and its shape decides how much noise it lets in.
    receiver_width_hz: f64,
    noise_bandwidth_hz: f64,
    /// In Contest mode, how the run goes; none in group training. A run of
    /// single callers has the band around it and no pile-up; a pile-up has
    /// its callers, spread as far as the spread and the filter let them,
    /// with no wanted station for them to sit under.
    run: Option<RunStyle>,
    /// How many pile-up callers the receiver has room for at once.
    fits: usize,
    /// In Contest mode, how the callers operate, and the line that says
    /// what each habit does in the contest chosen; none in group training.
    habits: Option<(CallerHabits, String)>,
    /// In Contest mode, whether some callers flutter; none in group
    /// training.
    flutter: Option<bool>,
}

impl Band {
    fn of(settings: &TrainingSettings) -> Self {
        let receiver = FilterDesign::from_settings(settings);
        let contest = settings.curriculum.char_set_mode == CharSetMode::Contest;
        Self {
            band: settings.band.clone(),
            receiver_width_hz: receiver.width_hz(),
            noise_bandwidth_hz: receiver.noise_bandwidth_hz(),
            run: contest.then_some(settings.contest.contest_style),
            fits: filter_capacity(settings),
            habits: contest.then(|| {
                let rules = settings.contest.contest_kind.rules();
                let line = habits_line(rules, settings.contest.side());
                (settings.contest.contest_habits, line)
            }),
            flutter: contest.then_some(settings.contest.contest_flutter),
        }
    }
}

#[component]
pub fn BandConditionsCard(
    settings: Signal<TrainingSettings>,
    previewing: bool,
    on_preview: EventHandler<()>,
    on_stop: EventHandler<()>,
) -> Element {
    let view = use_memo(move || Band::of(&settings.read()));
    let view = view.read();
    let b = &view.band;
    // As heard through the filter you have set, so narrowing it visibly buys
    // signal-to-noise — which is the whole reason to reach for it.
    let noise_reading = if b.noise_enabled && b.noise_level > 0.0 {
        format!(
            "S/N {:.0} dB",
            heard_snr_db(b.noise_level, view.noise_bandwidth_hz)
        )
    } else {
        "Off".to_string()
    };
    let widened = (view.receiver_width_hz > b.filter_width_hz + 0.5).then(|| {
        format!(
            "{:.0} Hz — widened to {:.0} Hz to pass your pitch range",
            b.filter_width_hz, view.receiver_width_hz
        )
    });
    // Rough habits bring QRM whatever the band's own traffic, so the hint
    // of a run of single callers cannot say its band is empty at 1, nor a
    // pile-up's that one caller is all you hear.
    let rough = matches!(view.habits, Some((CallerHabits::Rough, _)));
    let callers = if b.stations_max == STATIONS_MIN {
        "caller"
    } else {
        "callers"
    };
    // A run of single callers has the band around it, and no pile-up; a
    // pile-up's range is its callers.
    let (stations, unit, stations_hint) = match view.run {
        Some(RunStyle::Single) if rough => (
            "Band activity",
            "at once",
            "How busy the band is around your run, your caller counted. At 1 there is nobody on it but your caller and, with Rough habits, now and then a station that is not calling you; above that other stations work each other at their own pitches and speeds, never more of them keying at once than the rest of the count.",
        ),
        Some(RunStyle::Single) => (
            "Band activity",
            "at once",
            "How busy the band is around your run, your caller counted. At 1 there is nobody on it but your caller; above that other stations work each other at their own pitches and speeds, never more of them keying at once than the rest of the count.",
        ),
        Some(RunStyle::Pileup) if rough => (
            "Callers at once",
            callers,
            "How many call you after each CQ or TU, drawn afresh each time; those you do not work keep calling while their patience lasts. Hold both ends together for the same number every time; at 1 there is no pile-up, only one caller at a time, and with Rough habits now and then a station on your frequency that is not calling you.",
        ),
        Some(RunStyle::Pileup) => (
            "Callers at once",
            callers,
            "How many call you after each CQ or TU, drawn afresh each time; those you do not work keep calling while their patience lasts. Hold both ends together for the same number every time; at 1 there is no pile-up, only one caller at a time.",
        ),
        None => (
            "Stations calling",
            "at once",
            "How many call at once, yours included, drawn afresh for each group. At 1 the band is just your station; above that the others call across it, and the band has its own traffic around it — never more than the rest of the count keying at once. Hold both ends together for the same number every time, or open them up so you never know what you are walking into.",
        ),
    };
    let fits = format!("Your filter fits {}", counted(view.fits, "caller"));
    // Pile-up callers spread out around your pitch, and so do a group's
    // others around the station you want, which they sit under too.
    let pileup = view.run == Some(RunStyle::Pileup);
    let group_pileup = view.run.is_none() && b.stations_max > STATIONS_MIN;
    rsx! {
        div { class: "card stack-sm",
            CardHead {
                icon: IconName::Waves,
                title: "Band conditions",
                note: if view.habits.is_some() {
                    "Stations, callers' habits, fading and flutter, static, noise and the receiver that hears them"
                } else {
                    "Stations, fading, static, noise and the receiver that hears them"
                },
                help: help::band(),
                if previewing {
                    button {
                        id: control_id("btn", "band stop"),
                        class: "btn btn-secondary btn-sm",
                        onclick: move |_| on_stop.call(()),
                        Icon { name: IconName::Stop }
                        "Stop"
                    }
                } else {
                    button {
                        id: control_id("btn", "band preview"),
                        class: "btn btn-primary btn-sm",
                        onclick: move |_| on_preview.call(()),
                        Icon { name: IconName::Play }
                        "Live preview"
                    }
                }
                SectionReset {
                    label: "band conditions",
                    on_reset: move |()| {
                        settings.write().reset_section(SettingsSection::BandConditions);
                    },
                }
            }
            if previewing {
                p { class: "muted", style: "margin: 0;",
                    "Looping “CQ” with the current mix. Move a slider and you hear it immediately."
                }
            }
            div { class: "field-grid",
                SliderField {
                    label: "Filter width (-6 dB)".to_string(),
                    id: "slider-filter-width".to_string(),
                    value_label: format!("{:.0} Hz", b.filter_width_hz),
                    value: b.filter_width_hz,
                    min: FILTER_WIDTH_MIN,
                    max: FILTER_WIDTH_MAX,
                    step: 10.0,
                    disabled: false,
                    onchange: move |v| settings.write().band.filter_width_hz = v,
                }
            }
            if let Some(widened) = widened {
                p { id: "filter-widened", class: "muted", style: "margin: 0;", "{widened}" }
            }
            div { class: "field",
                span { class: "field-label", "Filter shape" }
                div { class: "segmented",
                    for (shape, label) in FILTER_SHAPES {
                        Seg {
                            label,
                            id: control_id("seg", &format!("filter {label}")),
                            active: b.filter_shape == shape,
                            onclick: move |_| settings.write().band.filter_shape = shape,
                        }
                    }
                }
            }
            p { class: "muted", style: "margin: 0;",
                "Everything you hear comes through this. Narrow it and less noise gets in and the filter rings longer, but a station off your pitch fades with it. Sharp: a modern DSP filter, flat with steep sides. Soft: rounded like an older crystal filter. Peaked: Sharp plus the rig's audio peak filter — a resonance at your pitch; stations off your pitch come out a few dB weaker."
            }
            SettingsRange {
                settings,
                which: RangeSetting::Stations,
                label: stations,
                unit,
                min_bound: f64::from(STATIONS_MIN),
                max_bound: f64::from(STATIONS_MAX),
                step: 1.0,
                hint: stations_hint,
            }
            if pileup {
                p { id: "filter-fits", class: "muted", style: "margin: 0;",
                    "{fits}: they sit either side of your pitch, as far out as the spread, at least 60 Hz apart, and only where your filter passes them. Narrow it and fewer fit; the rest wait their turn."
                }
            }
            if group_pileup {
                p { class: "muted", style: "margin: 0;",
                    "The others sit either side of the one you want, further out and a good way under it, and only ever where your filter passes them, out onto its skirts — copy the strongest, and narrow the filter until the rest drop away. A narrow filter leaves them little room, so it thins the pile-up: not everyone who calls gets through. Between groups the band carries on: other stations working each other at their own pitches and speeds, coming and going."
                }
            }
            if pileup || group_pileup {
                div { class: "field-grid",
                    SliderField {
                        label: "Pile-up spread".to_string(),
                        value_label: format!("±{:.0} Hz", b.pileup_spread_hz),
                        value: b.pileup_spread_hz,
                        min: PILEUP_SPREAD_MIN,
                        max: PILEUP_SPREAD_MAX,
                        step: 5.0,
                        disabled: false,
                        onchange: move |v| settings.write().band.pileup_spread_hz = v,
                    }
                    if group_pileup {
                        SliderField {
                            label: "Pile-up is weaker by".to_string(),
                            value_label: format!("{:.0} dB", b.pileup_level_db),
                            value: b.pileup_level_db,
                            min: PILEUP_LEVEL_MIN_DB,
                            max: PILEUP_LEVEL_MAX_DB,
                            step: 1.0,
                            disabled: false,
                            onchange: move |v| settings.write().band.pileup_level_db = v,
                        }
                    }
                }
            }
            if let Some((habits, line)) = &view.habits {
                div { class: "field",
                    span { class: "field-label", "Caller habits" }
                    div { class: "segmented",
                        for (each, label) in CALLER_HABITS {
                            Seg {
                                label,
                                id: control_id("habits", label),
                                active: *habits == each,
                                onclick: move |_| settings.write().contest.contest_habits = each,
                            }
                        }
                    }
                }
                p { id: "habits-line", class: "muted", style: "margin: 0;", "{line}" }
            }
            Switch {
                title: "QSB fading".to_string(),
                description: "Slow gain swells on the Morse signal only.".to_string(),
                checked: b.qsb_enabled,
                onchange: move |on| settings.write().band.qsb_enabled = on,
            }
            div { class: "field-grid",
                SliderField {
                    label: "Depth".to_string(),
                    value_label: format!("{:.0}%", b.qsb_depth * 100.0),
                    value: b.qsb_depth,
                    min: 0.0,
                    max: 1.0,
                    step: 0.05,
                    disabled: !b.qsb_enabled,
                    onchange: move |v| settings.write().band.qsb_depth = v,
                }
                SliderField {
                    label: "Rate".to_string(),
                    value_label: format!("{:.2} Hz", b.qsb_rate_hz),
                    value: b.qsb_rate_hz,
                    min: QSB_RATE_MIN_HZ,
                    max: QSB_RATE_MAX_HZ,
                    step: 0.01,
                    disabled: !b.qsb_enabled,
                    onchange: move |v| settings.write().band.qsb_rate_hz = v,
                }
            }
            if let Some(flutter) = view.flutter {
                Switch {
                    title: "Flutter".to_string(),
                    description: "Three callers in ten flutter, their signal rising and falling 3 to 33 times a second, as over a polar path, fading or not. Your own message never does.".to_string(),
                    checked: flutter,
                    onchange: move |on| settings.write().contest.contest_flutter = on,
                }
            }
            Switch {
                title: "QRN static".to_string(),
                description: "Lightning: distant crackle and the crash of a near storm.".to_string(),
                checked: b.qrn_enabled,
                onchange: move |on| settings.write().band.qrn_enabled = on,
            }
            SliderField {
                label: "Intensity".to_string(),
                id: "slider-qrn-intensity".to_string(),
                value_label: format!("{:.0}%", b.qrn_level * 100.0),
                value: b.qrn_level,
                min: 0.0,
                max: 1.0,
                step: 0.05,
                disabled: !b.qrn_enabled,
                onchange: move |v| settings.write().band.qrn_level = v,
            }
            Switch {
                title: "Band noise".to_string(),
                description: "The steady hiss of the band, through your filter.".to_string(),
                checked: b.noise_enabled,
                onchange: move |on| settings.write().band.noise_enabled = on,
            }
            SliderField {
                label: "Level".to_string(),
                id: "slider-noise-level".to_string(),
                value_label: noise_reading,
                value: b.noise_level,
                min: 0.0,
                max: 1.0,
                step: NOISE_LEVEL_PER_DB,
                disabled: !b.noise_enabled,
                onchange: move |v| settings.write().band.noise_level = v,
            }
            Switch {
                title: "AGC".to_string(),
                description: "The receiver rides its gain: the band ducks under a strong station and behind a crash.".to_string(),
                checked: b.agc_enabled,
                onchange: move |on| settings.write().band.agc_enabled = on,
            }
        }
    }
}
