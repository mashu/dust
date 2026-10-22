//! The mixer, listened to the way the stream plays it.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::atomic::AtomicUsize;

use super::*;
use cw_core::band::{BandMixer, FilterDesign, Qsb};
use cw_core::{
    CALLSIGN_TIER_MAX, FILTER_WIDTH_MIN, MONITOR_LEVEL, PlannedTransmission, STATIONS_MAX, Segment,
    Sender, SplitMix64, StationVoice, TrainingSettings, Transmission, Turn, VOLUME_MAX, VOLUME_MIN,
    generate_callsign, plan_morse_playback_for, plan_transmission, plan_turn, resolve_station,
};

/// Low enough to keep the long listens quick; the band sounds the same at
/// any rate.
const SR: u32 = 16_000;

/// The default band with its static off: the steady hiss, whose level is
/// the thing to hear a seam in.
fn hiss() -> TrainingSettings {
    let mut settings = TrainingSettings::default();
    settings.band.qrn_enabled = false;
    settings.band.qsb_enabled = false;
    settings
}

fn band(settings: &TrainingSettings, seed: u64) -> BandVoice {
    BandVoice::new(BandMixer::new(SR, settings, seed), SR)
}

fn send(settings: &TrainingSettings, text: &str) -> SendVoice {
    let voice = resolve_station(settings, &mut SplitMix64::new(5));
    let plan = plan_transmission(&Transmission::alone(text, voice), settings);
    SendVoice::new(&plan, &FilterDesign::from_settings(settings), None, SR, 0.0)
}

fn render(mixer: &mut Mixer, frames: usize) -> Vec<f32> {
    let mut out = vec![0.0f32; frames];
    mixer.render(&mut out, 0);
    out
}

fn power(samples: &[f32]) -> f64 {
    samples.iter().map(|s| f64::from(*s).powi(2)).sum::<f64>() / samples.len() as f64
}

fn db(ratio: f64) -> f64 {
    10.0 * ratio.log10()
}

/// The furthest a waveform swings from nothing.
fn loudest(samples: &[f32]) -> f32 {
    samples.iter().fold(0.0, |a, s| a.max(s.abs()))
}

/// The biggest change between neighbouring samples: where a waveform is cut
/// off, or a layer drops out, it steps.
fn biggest_step(samples: &[f32]) -> f32 {
    samples
        .windows(2)
        .map(|w| (w[1] - w[0]).abs())
        .fold(0.0, f32::max)
}

/// A band rebuilt — every slider move in the band preview does it — comes
/// in without a hole in the hiss. Switched in cold, a new band's filter was
/// empty and its AGC's look-ahead full of silence, and the old one faded out
/// under it: some ten decibels gone for a few milliseconds on every rebuild.
/// A straight-line crossfade of two unrelated noises sags 3 dB on its own.
///
/// Averaged over many rebuilds, in 5 ms windows, against the hiss before
/// each one.
#[test]
fn a_new_band_comes_in_without_a_hole_in_the_hiss() {
    let settings = hiss();
    let mut mixer = Mixer::new(SR);
    let mut swept = Swept::new();
    mixer.set_band(Some(band(&settings, 1)), &mut swept);
    render(&mut mixer, SR as usize / 2);
    let window = SR as usize / 200;
    let windows = 10;
    let swaps = 300;
    let (mut steady, mut around) = (0.0, vec![0.0; windows]);
    for k in 0..swaps {
        steady += power(&render(&mut mixer, window * windows));
        let mut swept = Swept::new();
        mixer.set_band(Some(band(&settings, 100 + k)), &mut swept);
        let after = render(&mut mixer, window * windows);
        for (w, chunk) in after.chunks(window).enumerate() {
            around[w] += power(chunk);
        }
    }
    let levels: Vec<f64> = around.iter().map(|p| db(p / steady)).collect();
    assert!(
        levels.iter().all(|level| level.abs() < 1.0),
        "the hiss through a rebuild, in dB against before it: {levels:.2?}"
    );
}

/// Bands rebuilt faster than one can fade out each fade out in full: none is
/// dropped part of the way down, which is a step in the waveform.
#[test]
fn bands_rebuilt_in_quick_succession_all_fade_out() {
    let settings = hiss();
    let mut mixer = Mixer::new(SR);
    let mut swept = Swept::new();
    mixer.set_band(Some(band(&settings, 1)), &mut swept);
    let steady = biggest_step(&render(&mut mixer, SR as usize * 2));
    let mut worst = 0.0f32;
    for k in 0..40 {
        let mut through = Vec::new();
        for j in 0..3 {
            let mut swept = Swept::new();
            mixer.set_band(Some(band(&settings, 10 * k + j)), &mut swept);
            through.extend(render(&mut mixer, SR as usize / 500));
        }
        through.extend(render(&mut mixer, SR as usize / 20));
        worst = worst.max(biggest_step(&through));
    }
    assert!(
        worst < 1.5 * steady,
        "rebuilds stepped by {worst}, against {steady} in the hiss itself"
    );
}

/// A send started while the last one is still sounding lets the last one
/// ride down over its release rather than cutting it off — whatever point
/// of its cycle it was at.
#[test]
fn a_new_send_lets_the_last_one_ride_out_its_release() {
    let settings = hiss();
    for offset in 0..6 {
        let mut mixer = Mixer::new(SR);
        let mut swept = Swept::new();
        mixer.start(send(&settings, "T"), &mut swept);
        // Into the middle of the dah, at a different point of its cycle
        // each time.
        let into = render(&mut mixer, SR as usize / 10 + 3 * offset);
        assert!(into.iter().any(|s| s.abs() > 0.1), "the dah is sounding");
        mixer.start(send(&settings, "T"), &mut swept);
        let mut across = vec![into[into.len() - 1]];
        across.extend(render(&mut mixer, SR as usize / 50));
        // A 600 Hz tone at full level moves at most a quarter of its height
        // between samples at this rate; a cut is a step of all of it.
        let step = biggest_step(&across);
        assert!(step < 0.1, "offset {offset}: a step of {step}");
    }
}

/// A send stopped part way leaves nothing of itself in the receiver: the
/// band comes back up as if it had never been ducked. A send stopped mid-way
/// used to leave its level with the AGC for good, and the band sat 4 dB
/// down until the next send overwrote it.
#[test]
fn a_stopped_send_leaves_the_band_as_it_was() {
    let settings = hiss();
    let seconds = |s: f64| (s * f64::from(SR)) as usize;
    let (mut heard, mut untouched) = (Mixer::new(SR), Mixer::new(SR));
    let mut swept = Swept::new();
    heard.set_band(Some(band(&settings, 7)), &mut swept);
    untouched.set_band(Some(band(&settings, 7)), &mut swept);
    heard.start(send(&settings, "TTTTTTTTTT"), &mut swept);
    render(&mut heard, seconds(0.5));
    render(&mut untouched, seconds(0.5));
    heard.stop();
    render(&mut heard, seconds(2.0));
    render(&mut untouched, seconds(2.0));
    let (a, b) = (
        render(&mut heard, seconds(1.0)),
        render(&mut untouched, seconds(1.0)),
    );
    let level = db(power(&a) / power(&b));
    assert!(
        level.abs() < 0.2,
        "two seconds after the stop the band was {level:.2} dB off"
    );
}

/// A send is over once its last sample has been heard, `latency` frames
/// after it was played.
#[test]
fn a_send_finishes_when_it_has_been_heard() {
    let settings = hiss();
    let mut mixer = Mixer::new(SR);
    let mut swept = Swept::new();
    let voice = send(&settings, "E");
    let finished = voice.finished_flag();
    mixer.start(voice, &mut swept);
    let latency = SR as usize / 10;
    let mut played = 0;
    while !finished.load(Ordering::Acquire) && played < SR as usize * 2 {
        let mut out = vec![0.0f32; 64];
        mixer.render(&mut out, latency);
        played += 64;
    }
    let expected = send(&settings, "E");
    let mut alone = Mixer::new(SR);
    let flag = expected.finished_flag();
    alone.start(expected, &mut swept);
    let mut unheard = 0;
    while !flag.load(Ordering::Acquire) && unheard < SR as usize * 2 {
        let mut out = vec![0.0f32; 64];
        alone.render(&mut out, 0);
        unheard += 64;
    }
    assert!(
        played >= unheard + latency && played <= unheard + latency + 64,
        "finished after {played} frames, {unheard} with no latency"
    );
    // And the next send is reckoned heard as far behind as the speaker was.
    assert!((mixer.latency_sec() - 0.1).abs() < 1e-9);
    assert_eq!(alone.latency_sec(), 0.0);
}

/// A keyer at `tone_hz` and `volume`, at 25 WPM.
fn keyer(tone_hz: f64, volume: f64) -> StationVoice {
    StationVoice {
        tone_hz,
        char_wpm: 25.0,
        effective_wpm: 25.0,
        volume,
        weight: 1.0,
        dash_ratio: 3.0,
    }
}

/// `who` sending `text` in a contest's turn, from `start_sec`.
fn segment(
    settings: &TrainingSettings,
    who: Sender,
    text: &str,
    voice: StationVoice,
    start_sec: f64,
) -> Segment {
    let end_sec = start_sec + plan_morse_playback_for(text, settings, &voice).duration_sec;
    Segment {
        who,
        text: text.into(),
        voice,
        start_sec,
        end_sec,
        fade: None,
    }
}

/// A contest's turn, as the run plans it.
fn planned_turn(settings: &TrainingSettings, segments: &[&Segment]) -> PlannedTransmission {
    let turn = Turn {
        id: 1,
        at_run_ms: 0,
        segments: segments.iter().map(|segment| (*segment).clone()).collect(),
    };
    plan_turn(&turn, settings)
}

/// What the mixer plays of `plan` over `band`, if there is one, from its
/// start to a little past its end.
fn hear(
    settings: &TrainingSettings,
    plan: &PlannedTransmission,
    band: Option<BandVoice>,
) -> Vec<f32> {
    let mut mixer = Mixer::new(SR);
    let mut swept = Swept::new();
    mixer.set_band(band, &mut swept);
    let receiver = FilterDesign::from_settings(settings);
    mixer.start(SendVoice::new(plan, &receiver, None, SR, 0.0), &mut swept);
    render(
        &mut mixer,
        ((plan.wanted.duration_sec + 0.1) * f64::from(SR)) as usize,
    )
}

/// Your message in a contest is not received, so the receiver's filter
/// takes none of it: sent a kilohertz off the narrowest filter, which all
/// but silences it through the receiver, it is heard at the monitor's level
/// with your sending at its loudest.
#[test]
fn your_message_is_not_filtered() {
    let mut settings = hiss();
    settings.contest.my_sending_db = 0;
    settings.band.filter_width_hz = FILTER_WIDTH_MIN;
    let off = settings.side_tone_center() + 1_000.0;
    let mine = segment(&settings, Sender::You, "TTT", keyer(off, 1.0), 0.0);
    let dry = planned_turn(&settings, &[&mine]);
    let monitored = f64::from(loudest(&hear(&settings, &dry, None)));
    assert!(
        (monitored - MONITOR_LEVEL).abs() < 0.02 * MONITOR_LEVEL,
        "your message peaked at {monitored}"
    );
    let received = PlannedTransmission {
        dry_wanted: false,
        ..dry
    };
    let filtered = f64::from(loudest(&hear(&settings, &received, None)));
    assert!(
        filtered < 0.1 * MONITOR_LEVEL,
        "through the receiver it peaked at {filtered}"
    );
}

/// Your sending sets the level your message is heard at, the monitor's
/// turned down by its decibels, the dry sample and nothing else: the
/// replies play as they would at any level of it; off, your message is
/// silence, and the send lasts as long, its reply where it was.
#[test]
fn your_sending_sets_the_dry_level_alone() {
    let mut settings = hiss();
    let centre = settings.side_tone_center();
    let mine = segment(&settings, Sender::You, "TTT", keyer(centre, 1.0), 0.0);
    let reply = segment(
        &settings,
        Sender::Caller(0),
        "K5XYZ",
        keyer(centre, 1.0),
        mine.end_sec + 0.2,
    );
    let mut loudest_reply = None;
    for db in [0, -12, -30, cw_core::MY_SENDING_OFF_DB] {
        settings.contest.my_sending_db = db;
        let level = settings.contest.sending_level();
        let yours = planned_turn(&settings, &[&mine]);
        let heard = f64::from(loudest(&hear(&settings, &yours, None)));
        if db == cw_core::MY_SENDING_OFF_DB {
            assert_eq!(heard, 0.0, "off is silence");
        } else {
            assert!(
                (heard - level).abs() < 0.02 * level,
                "{db} dB: {heard} for {level}"
            );
        }
        let turn = planned_turn(&settings, &[&mine, &reply]);
        let played = hear(&settings, &turn, None);
        let sr = f64::from(SR);
        let from = (reply.start_sec * sr) as usize;
        let tail = played[from..].to_vec();
        match &loudest_reply {
            None => loudest_reply = Some((played.len(), tail)),
            Some((length, before)) => {
                assert_eq!(played.len(), *length, "{db} dB: the send's length");
                assert_eq!(&tail, before, "{db} dB: the reply as ever");
            }
        }
    }
}

/// The AGC never hears your message, so the weakest reply right after it
/// comes in as it would after silence: over the reply's first 0.3 s the
/// receiver plays exactly what it plays for the reply alone. Through the
/// receiver, as your message used to go, it ducked them.
#[test]
fn your_message_never_ducks_the_reply_after_it() {
    let settings = hiss();
    let centre = settings.side_tone_center();
    let mine = segment(
        &settings,
        Sender::You,
        "K5XYZ 5NN TT7",
        keyer(centre, 1.0),
        0.0,
    );
    let weak = keyer(centre, VOLUME_MIN);
    let reply = segment(
        &settings,
        Sender::Caller(0),
        "TU 5NN 214",
        weak,
        mine.end_sec + 0.1,
    );
    let sr = f64::from(SR);
    let first = (reply.start_sec * sr) as usize..((reply.start_sec + 0.3) * sr) as usize;
    let on_band = || Some(band(&settings, 3));

    let after = hear(
        &settings,
        &planned_turn(&settings, &[&mine, &reply]),
        on_band(),
    );
    let alone = hear(&settings, &planned_turn(&settings, &[&reply]), on_band());
    assert_eq!(after[first.clone()], alone[first.clone()]);

    let mut received = planned_turn(&settings, &[&mine, &reply]);
    received.dry_wanted = false;
    let ducked = hear(&settings, &received, on_band());
    let level = db(power(&ducked[first.clone()]) / power(&alone[first]));
    assert!(
        level < -1.0,
        "through the receiver the reply came in {level:.2} dB down"
    );
}

/// Your message joins after the AGC, not under its gain: on a band whose
/// static, and a caller at full level calling across you, pull the gain
/// well down, what your message adds to the receiver's output is its own
/// tones at your sending's level, sample for sample. Ridden by the gain, it
/// would duck on every crash.
#[test]
fn your_message_is_not_ridden_by_the_agc() {
    let mut settings = TrainingSettings::default();
    settings.band.qsb_enabled = false;
    settings.band.qrn_level = 1.0;
    let centre = settings.side_tone_center();
    let mine = segment(
        &settings,
        Sender::You,
        "K5XYZ 5NN TT7",
        keyer(centre, 1.0),
        0.0,
    );
    let loud = segment(
        &settings,
        Sender::Caller(0),
        "K5XYZ K5XYZ K5XYZ",
        keyer(centre, VOLUME_MAX),
        0.2,
    );
    assert!(loud.end_sec > mine.end_sec);
    let both = planned_turn(&settings, &[&mine, &loud]);
    let callers = planned_turn(&settings, &[&loud]);
    let on_band = || Some(band(&settings, 3));

    let with = hear(&settings, &both, on_band());
    let without = hear(&settings, &callers, on_band());
    let alone = PlannedTransmission {
        others: Vec::new(),
        ..both
    };
    let yours = hear(&settings, &alone, None);
    assert_eq!(with.len(), without.len());
    for (i, ((a, b), y)) in with.iter().zip(&without).zip(&yours).enumerate() {
        assert!((a - b - y).abs() < 1e-6, "at {i}: {} against {y}", a - b);
    }

    // The gain it was not under, as the band rides the caller alone.
    let mut ridden = band(&settings, 3);
    let receiver = FilterDesign::from_settings(&settings);
    let mut voice = SendVoice::new(&callers, &receiver, None, SR, 0.0);
    ridden.reserve_stations(voice.callers_now());
    let lowest = (0..(mine.end_sec * f64::from(SR)) as usize)
        .map(|_| ridden.next(voice.next().heard).gain)
        .fold(1.0, f64::min);
    assert!(
        lowest < 0.5,
        "under your message the gain fell only to {lowest}"
    );
}

/// A turn stretched to its last reply plays until that reply has been
/// heard, though your message ended long before it: dry or not, a send is
/// as long as its plan.
#[test]
fn a_stretched_turn_finishes_once_its_last_reply_is_heard() {
    let settings = hiss();
    let centre = settings.side_tone_center();
    let mine = segment(&settings, Sender::You, "TU", keyer(centre, 1.0), 0.0);
    let slow = StationVoice {
        char_wpm: 12.0,
        effective_wpm: 12.0,
        ..keyer(centre, 1.0)
    };
    let reply = segment(
        &settings,
        Sender::Caller(0),
        "K5XYZ",
        slow,
        mine.end_sec + 0.3,
    );
    let dry = planned_turn(&settings, &[&mine, &reply]);
    assert!(dry.wanted.duration_sec > mine.end_sec + 1.0);
    let latency = SR as usize / 10;
    let finishing = |plan: &PlannedTransmission| {
        let mut mixer = Mixer::new(SR);
        let mut swept = Swept::new();
        let voice = SendVoice::new(plan, &FilterDesign::from_settings(&settings), None, SR, 0.0);
        let finished = voice.finished_flag();
        mixer.start(voice, &mut swept);
        let mut played = Vec::new();
        while !finished.load(Ordering::Acquire) && played.len() < SR as usize * 10 {
            let mut out = vec![0.0f32; 64];
            mixer.render(&mut out, latency);
            played.extend(out);
        }
        played
    };

    let played = finishing(&dry);
    let sr = f64::from(SR);
    let ends = (dry.wanted.duration_sec * sr).ceil() as usize;
    assert!(
        played.len() >= ends + latency && played.len() <= ends + latency + SR as usize / 10,
        "finished after {} frames, its reply heard by {}",
        played.len(),
        ends + latency
    );
    let last = dry.others[0].events.last().expect("a reply");
    let from = (last.start_sec * sr) as usize;
    assert!(loudest(&played[from..ends]) > 0.1, "the reply was cut");
    let received = PlannedTransmission {
        dry_wanted: false,
        ..dry
    };
    assert_eq!(finishing(&received).len(), played.len());
}

/// The output stage catches what is too loud rather than squaring it off:
/// the same curve the band is calibrated through, applied once to the sum.
#[test]
fn the_output_stage_catches_rather_than_clips() {
    assert_eq!(finish(0.5), 0.5, "below the knee it does nothing");
    assert_eq!(finish(-0.5), -0.5);
    assert!(finish(1.4) > 0.9 && finish(1.4) < 1.0);
    assert!(finish(2.0) > finish(1.4), "limiting, not clipping flat");
    assert!(finish(-3.0) < -0.9 && finish(-3.0) >= -1.0);
    assert_eq!(finish(f32::NAN), 0.0);
    assert_eq!(finish(f32::INFINITY), 0.0);
}

/// The audio thread never waits: a callback that finds the UI holding the
/// mixer goes without it for one buffer, and the paddle's sidetone, which
/// needs no lock, plays regardless. A lock poisoned elsewhere is still a
/// good one.
#[test]
fn the_stream_never_waits_for_the_mixer() {
    let settings = hiss();
    let mixer = Arc::new(Mutex::new(Mixer::new(SR)));
    let mut swept = Swept::new();
    mixer
        .lock()
        .expect("mixer")
        .start(send(&settings, "TTT"), &mut swept);
    let control = Arc::new(SidetoneControl::new());
    let failed = Arc::new(AtomicBool::new(false));
    let mut output = Output::new(
        Arc::clone(&mixer),
        Arc::clone(&control),
        Arc::clone(&failed),
        SR,
    );

    control.set(crate::audio::shape::ToneRequest::new(true, 700.0, 0.2));
    let mut out = vec![0.0f32; 512];
    {
        let _held = mixer.lock().expect("mixer");
        output.fill(&mut out, 0);
    }
    let peak = out.iter().fold(0.0f32, |a, s| a.max(s.abs()));
    assert!(
        peak > 0.15 && peak <= 0.2 + 1e-6,
        "the sidetone alone, peak {peak}"
    );

    let poisoner = Arc::clone(&mixer);
    let _ = std::thread::spawn(move || {
        let _guard = poisoner.lock();
        panic!("poison the lock");
    })
    .join();
    assert!(mixer.is_poisoned());
    control.off();
    let mut out = vec![0.0f32; SR as usize / 5];
    output.fill(&mut out, 0);
    assert!(
        out.iter().skip(SR as usize / 50).any(|s| s.abs() > 0.05),
        "the send still plays through a poisoned lock"
    );
    assert!(!failed.load(Ordering::Acquire));
}

/// The system's allocator, handed every call unchanged, which also counts
/// what a thread allocates and frees while that thread asks it to. It is the
/// allocator of the whole test binary — there can only be one — so to every
/// other test it is the system's in all but one load a call.
struct Counting;

/// What a counted thread has allocated and freed since it asked.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Counts {
    allocations: usize,
    frees: usize,
}

/// How many threads are being counted. Almost always none, and then a call
/// goes straight on to the system. Relaxed is enough: a thread always sees
/// its own count go up, and only its own calls are counted.
static COUNTING: AtomicUsize = AtomicUsize::new(0);

std::thread_local! {
    /// This thread's counts, while it is being counted. Const, and with
    /// nothing to drop, it is a plain thread-local static: reading it
    /// allocates nothing, which is what lets the allocator read it.
    static COUNTED: Cell<Option<Counts>> = const { Cell::new(None) };
}

/// Count one call on this thread, if it is being counted. Never panics, as
/// an allocator must not.
fn note(count: impl FnOnce(&mut Counts)) {
    if COUNTING.load(Ordering::Relaxed) == 0 {
        return;
    }
    let _ = COUNTED.try_with(|counted| {
        if let Some(mut counts) = counted.get() {
            count(&mut counts);
            counted.set(Some(counts));
        }
    });
}

// Safety: every call goes to the system allocator as it came, and the
// counting only touches this thread's own cell.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        note(|counts| counts.allocations += 1);
        // Safety: the caller's guarantees, passed on as they are.
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        note(|counts| counts.allocations += 1);
        // Safety: as for `alloc`.
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // In place or moved, it is the allocator's work: counted as a new
        // block and the old one freed.
        note(|counts| {
            counts.allocations += 1;
            counts.frees += 1;
        });
        // Safety: as for `alloc`.
        unsafe { System.realloc(ptr, layout, new_size) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        note(|counts| counts.frees += 1);
        // Safety: as for `alloc`.
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

/// What `body` allocated and freed on this thread.
fn counted(body: impl FnOnce()) -> Counts {
    COUNTED.with(|counted| counted.set(Some(Counts::default())));
    COUNTING.fetch_add(1, Ordering::Relaxed);
    body();
    COUNTING.fetch_sub(1, Ordering::Relaxed);
    COUNTED.with(Cell::take).unwrap_or_default()
}

/// A pile-up, as a session sends it: the station being copied, with every
/// other station the settings allow calling across it, through the receiver
/// and under the fading. `dry`, the station being copied is played past the
/// receiver, as your message in a contest is.
fn pile_up(settings: &TrainingSettings, rng: &mut SplitMix64, at_sec: f64, dry: bool) -> SendVoice {
    let voice = resolve_station(settings, rng);
    let call = Transmission::calling(settings, "CQ TEST", voice, rng, |rng| {
        generate_callsign(CALLSIGN_TIER_MAX, rng)
    });
    let mut plan = plan_transmission(&call, settings);
    plan.dry_wanted = dry;
    SendVoice::new(
        &plan,
        &FilterDesign::from_settings(settings),
        Qsb::of(&settings.band),
        SR,
        at_sec,
    )
}

/// A change made the way the player makes one: under the lock, with what it
/// sweeps let go of only once the lock is released.
fn on_ui(mixer: &Mutex<Mixer>, change: impl FnOnce(&mut Mixer, &mut Swept)) {
    let mut swept = Swept::new();
    change(&mut mixer.lock().expect("mixer"), &mut swept);
    drop(swept);
}

/// The audio callback allocates nothing and frees nothing, whatever the UI
/// does between its buffers. An allocator may wait on a lock of its own or
/// on the system, and a callback that waits is a hole in the sound.
///
/// A minute of a busy band — its stations, its static, a pile-up calling
/// over the sends, the fading on them, every other send with its wanted
/// station played past the receiver — while the UI rebuilds the band as a
/// dragged slider does and takes it away, starts sends over each other,
/// stops one and lets another play out, keys the paddle's sidetone, and now
/// and then holds the mixer through a buffer — and for a moment rebuilds the
/// band and starts a send on every one of a host's shortest buffers, with
/// more sends riding out their release than the mixer keeps room for.
/// Everything is built before the lock is taken and let go of after it, as
/// the player does it; the host hands over buffers of every size, up to the
/// third of a second a default one can.
#[test]
fn the_audio_callback_never_allocates_or_frees() {
    const SECONDS: usize = 60;
    const FRAMES: [usize; 6] = [64, 160, 256, 441, 1_024, 5_333];
    const SHORTEST: usize = 32;
    const LATENCY: usize = 512;

    // Without this the rest would pass for nothing.
    let freed = counted(|| drop(std::hint::black_box(vec![0u8; 16])));
    assert_eq!(
        freed,
        Counts {
            allocations: 1,
            frees: 1
        },
        "the counting counts"
    );

    let mut settings = TrainingSettings::default();
    settings.band.stations_max = STATIONS_MAX;
    settings.band.qrn_level = 1.0;
    let settings = settings.clamp();
    let mixer = Arc::new(Mutex::new(Mixer::new(SR)));
    let control = Arc::new(SidetoneControl::new());
    let failed = Arc::new(AtomicBool::new(false));
    let mut output = Output::new(
        Arc::clone(&mixer),
        Arc::clone(&control),
        Arc::clone(&failed),
        SR,
    );
    let mut rng = SplitMix64::new(11);
    // Made once, as the stream's own scratch buffer is.
    let mut buffer = vec![0.0f32; FRAMES.into_iter().max().unwrap_or(0)];
    let mut sends = Vec::new();
    let (mut played, mut peak) = (0, 0.0f32);
    let (mut total, mut first) = (Counts::default(), None);
    let mut k = 0;
    while played < SECONDS * SR as usize {
        // Every quarter of a minute or so: a slider dragged, the band
        // rebuilt on every buffer; a send every half second or so, each over
        // the last, and one of them stopped; a burst of both on every one of
        // a run of the shortest buffers, bands replaced before they are in
        // and sends faster than they can ride out; then the last send left
        // to play out, and the band taken away a while.
        let cycle = k % 200;
        let burst = (70..100).contains(&cycle);
        if burst || cycle < 20 || cycle % 13 == 0 {
            let next = (cycle != 130).then(|| band(&settings, k as u64));
            on_ui(&mixer, |mixer, swept| mixer.set_band(next, swept));
        }
        if burst || (cycle < 100 && cycle % 8 == 0) {
            let at_sec = played as f64 / f64::from(SR);
            let voice = pile_up(&settings, &mut rng, at_sec, sends.len() % 2 == 0);
            sends.push(voice.finished_flag());
            on_ui(&mixer, |mixer, swept| mixer.start(voice, swept));
        }
        if cycle == 52 {
            mixer.lock().expect("mixer").stop();
        }
        if k % 3 == 0 {
            let pitch = 500.0 + 50.0 * (k % 7) as f64;
            control.set(crate::audio::shape::ToneRequest::new(
                k % 6 == 0,
                pitch,
                0.2,
            ));
        }

        let frames = if burst {
            SHORTEST
        } else {
            FRAMES[k % FRAMES.len()]
        };
        let out = &mut buffer[..frames];
        let held = (k % 50 == 7).then(|| mixer.lock().expect("mixer"));
        let counts = counted(|| output.fill(out, LATENCY));
        drop(held);

        if counts != Counts::default() && first.is_none() {
            first = Some(k);
        }
        total.allocations += counts.allocations;
        total.frees += counts.frees;
        peak = out.iter().fold(peak, |a, s| a.max(s.abs()));
        played += frames;
        k += 1;
    }

    assert_eq!(
        total,
        Counts::default(),
        "the callback allocated or freed, first in buffer {first:?}"
    );
    assert!(!failed.load(Ordering::Acquire));
    assert!(
        sends.iter().any(|done| done.load(Ordering::Acquire)),
        "no send was heard out"
    );
    assert!(peak > 0.1, "nothing much played: a peak of {peak}");
}
