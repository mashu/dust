//! One send, as the audio callback plays it: generated sample by sample from
//! its plan, never rendered ahead.

use std::ops::AddAssign;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use cw_core::band::{FilterDesign, Qsb, ReceiverFilter};
use cw_core::{PlannedTransmission, PlaybackPlan, ToneEvent};

use crate::audio::shape::{SEND_RELEASE_SEC, release_gain};
use crate::audio::wait::LATE_MS;

/// How long a send runs on after its last element ends: the receiver's
/// filter rings, and the ringing is part of the sound.
const TAIL_SEC: f64 = 0.05;

/// The end of that tail, faded out: even the narrowest filter is still
/// ringing faintly when the tail runs out, and stopping a waveform where it
/// stands is a step.
const TAIL_FADE_SEC: f64 = 0.005;

/// How often the fading's level is worked out, with a straight line between.
/// The fastest fade moves at a few hertz, so four sines a sample to follow it
/// would be waste; a millisecond apart, the line is the fade to a fiftieth
/// of a decibel even where it turns at the ends of its range.
const FADING_STEP_SEC: f64 = 0.001;

/// One station's elements, in order, and where the send has got to in them.
struct Station {
    elements: Vec<Element>,
    next: usize,
}

/// One tone, placed on the sample clock.
struct Element {
    start: usize,
    len: usize,
    angular_hz: f64,
    tone: ToneEvent,
}

impl Element {
    fn new(tone: &ToneEvent, sample_rate: f64) -> Self {
        Self {
            start: (tone.start_sec * sample_rate).round().max(0.0) as usize,
            len: ((tone.duration_sec * sample_rate).round().max(0.0) as usize).max(1),
            angular_hz: std::f64::consts::TAU * tone.frequency_hz,
            tone: tone.clone(),
        }
    }

    fn end(&self) -> usize {
        self.start + self.len
    }

    /// The tone `i` samples in, keyed by its envelope.
    fn sample(&self, i: usize, sample_rate: f64) -> f32 {
        let t = i as f64 / sample_rate;
        (self.angular_hz * t).sin() as f32 * envelope_gain(&self.tone, t)
    }
}

/// The keying envelope at `t` seconds into a tone, read off its curve.
fn envelope_gain(tone: &ToneEvent, t: f64) -> f32 {
    let curve = &tone.envelope;
    if curve.len() < 2 || tone.duration_sec <= 0.0 {
        return tone.target_gain as f32;
    }
    let rel = (t / tone.duration_sec).clamp(0.0, 1.0);
    let pos = rel * (curve.len() - 1) as f64;
    let i = pos.floor() as usize;
    let frac = (pos - i as f64) as f32;
    let a = curve.get(i).copied().unwrap_or(0.0);
    let b = curve.get(i + 1).copied().unwrap_or(a);
    a * (1.0 - frac) + b * frac
}

impl Station {
    fn new(plan: &PlaybackPlan, sample_rate: f64) -> Self {
        Self {
            elements: plan
                .events
                .iter()
                .map(|tone| Element::new(tone, sample_rate))
                .collect(),
            next: 0,
        }
    }

    /// This station's sound at sample `pos`. Called with `pos` rising one at
    /// a time, so the elements it has finished are never looked at again.
    fn sample(&mut self, pos: usize, sample_rate: f64) -> f32 {
        while self
            .elements
            .get(self.next)
            .is_some_and(|element| element.end() <= pos)
        {
            self.next += 1;
        }
        match self.elements.get(self.next) {
            Some(element) if element.start <= pos => {
                element.sample(pos - element.start, sample_rate)
            }
            _ => 0.0,
        }
    }
}

/// The fading on a send, followed a step at a time.
struct Fading {
    qsb: Qsb,
    /// Samples between the points it is worked out at.
    every: usize,
    level: f64,
    slope: f64,
    left: usize,
}

/// One sample of a send, for the two places the mixer adds it, and whether
/// the receiver is to be muted for it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SendSample {
    /// Through the receiver's filter and under the fading, as its AGC hears
    /// it.
    pub heard: f64,
    /// Past the receiver: your own message in a contest, at the level you
    /// monitor it at, for after the AGC.
    pub dry: f64,
    /// You are sending, without full break-in: the receiver's output is to
    /// be muted, after its AGC, as a rig that is not QSK mutes it.
    pub keyed: bool,
}

impl AddAssign for SendSample {
    fn add_assign(&mut self, other: Self) {
        self.heard += other.heard;
        self.dry += other.dry;
        self.keyed |= other.keyed;
    }
}

/// One send playing out: every station's tones, summed — interference is
/// addition — through the receiver's filter and under the fading; and, in a
/// contest, your own message beside them, which the receiver never hears.
///
/// Built on the UI thread from the plan, which is all it allocates; the
/// audio callback only reads it forward.
pub struct SendVoice {
    /// The stations the receiver hears.
    stations: Vec<Station>,
    /// Your message, when the plan says it is dry: played as a rig's
    /// sidetone is, neither filtered nor faded, so its level is the one the
    /// plan gave it.
    dry: Option<Station>,
    filter: ReceiverFilter,
    fading: Option<Fading>,
    sample_rate: f64,
    /// Where the send starts on the band's clock. Fading is a property of
    /// the path, not of the group: starting every send at the same point of
    /// the fade would give every group an identical one.
    started_at_sec: f64,
    /// Silence still to play before its first sample: a send that starts at
    /// a given time, a contest's continuation taking over from the send
    /// before as that one gives way.
    wait: usize,
    /// Samples played so far, its wait aside.
    pos: usize,
    /// While you send without full break-in, how many samples in the
    /// receiver is muted till.
    muted_until: Option<usize>,
    /// Where the sound ends: the last element, its ringing, and the fade.
    end: usize,
    tail_fade: usize,
    /// Where a stopped send begins its release, on its own clock: ahead of
    /// where it has got to for a send that a continuation takes over from.
    released_at: Option<usize>,
    /// It was told to stop where a send taking over from it starts: your
    /// message in it goes on muting the receiver to there, so the gate does
    /// not open between the two.
    hands_over: bool,
    release_len: usize,
    /// The most other stations calling across this send at once.
    callers: usize,
    finished: Arc<AtomicBool>,
    /// When its first sample was played, on the wall clock.
    first_played: Option<Instant>,
    /// Set once what it has played falls behind the wall clock since its
    /// first sample by more than [`LATE_MS`]: the stream was held up while
    /// it played, so it went out later than planned. The stream has no
    /// audio clock to report, so the send watches for itself.
    late: Arc<AtomicBool>,
}

impl SendVoice {
    pub fn new(
        plan: &PlannedTransmission,
        receiver: &FilterDesign,
        fading: Option<Qsb>,
        sample_rate: u32,
        started_at_sec: f64,
    ) -> Self {
        let sr = f64::from(sample_rate.max(1));
        let received = (!plan.dry_wanted).then_some(&plan.wanted);
        let stations: Vec<Station> = received
            .into_iter()
            .chain(&plan.others)
            .map(|station| Station::new(station, sr))
            .collect();
        let samples = |seconds: f64| (seconds * sr).round().max(1.0) as usize;
        let length = (plan.wanted.duration_sec.max(0.0) * sr).ceil() as usize;
        let at = |seconds: f64| (seconds.max(0.0) * sr).round() as usize;
        Self {
            stations,
            dry: plan.dry_wanted.then(|| Station::new(&plan.wanted, sr)),
            filter: ReceiverFilter::new(sample_rate, receiver),
            fading: fading.map(|qsb| Fading {
                qsb,
                every: samples(FADING_STEP_SEC),
                level: f64::from(qsb.gain_at(started_at_sec)),
                slope: 0.0,
                left: 0,
            }),
            sample_rate: sr,
            started_at_sec,
            wait: 0,
            pos: 0,
            muted_until: plan.muted_until.map(at),
            end: length + samples(TAIL_SEC),
            tail_fade: samples(TAIL_FADE_SEC),
            released_at: None,
            hands_over: false,
            release_len: samples(SEND_RELEASE_SEC),
            callers: plan.others_at_once(),
            finished: Arc::new(AtomicBool::new(false)),
            first_played: None,
            late: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Set once the send has been heard to its end, or its release has: what
    /// the waiter watches.
    pub fn finished_flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.finished)
    }

    /// Set once it is found to have played late ([`Self::settle`]).
    pub fn late_flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.late)
    }

    /// Start only after `samples` of silence: a send that starts at a given
    /// time.
    pub fn wait(&mut self, samples: usize) {
        self.wait = samples;
    }

    /// Stop: ride down to silence over the release rather than cutting the
    /// waveform where it stands, which is a click. Your sending is over, so
    /// the receiver comes back at once.
    pub fn release(&mut self) {
        self.hands_over = false;
        self.release_after(0);
    }

    /// Stop once `samples` more have played, its wait among them: where the
    /// send that takes over from it starts. One stopped before it starts is
    /// never heard at all; one stopped already stays stopped.
    fn release_after(&mut self, samples: usize) {
        if samples <= self.wait && self.wait > 0 {
            self.wait = 0;
            self.end = self.pos;
            return;
        }
        let at = self.pos + samples - self.wait;
        if at < self.end && self.released_at.is_none_or(|before| at < before) {
            self.released_at = Some(at);
        }
    }

    /// Stop so as to be silent once `samples` more have played: ridden down
    /// over the release before then, as a send that another takes over from
    /// there is, so the two never sound together. The machine puts that
    /// moment at the start of a character of your message, or half a word
    /// space after its last, and no sooner than the player's lead and a
    /// release after the key, so the ride down falls in a silence of yours
    /// and takes nothing of it. Your message in it goes on muting the
    /// receiver to the handover, where the next send's takes over.
    pub fn release_by(&mut self, samples: usize) {
        self.hands_over = true;
        self.release_after(samples.saturating_sub(self.release_len));
    }

    /// Whether it has been told to stop, now or ahead.
    pub fn is_released(&self) -> bool {
        self.released_at.is_some()
    }

    /// How many samples from now until it reaches `into` samples in: where
    /// a send that takes over from it at a given time starts. A moment
    /// within a release of the start of one that has played nothing yet is
    /// its start: a second key taking over from a continuation at its own
    /// start goes in its place exactly, not the moment of clock rounding
    /// after it in which neither would be sending.
    pub fn samples_until(&self, into: usize) -> usize {
        let into = if self.pos == 0 && into < self.release_len {
            0
        } else {
            into
        };
        (self.wait + into).saturating_sub(self.pos)
    }

    /// Where the sound ends, on the send's own sample clock.
    fn sound_ends_at(&self) -> usize {
        match self.released_at {
            Some(at) => (at + self.release_len).min(self.end),
            None => self.end,
        }
    }

    /// Whether anything of it is still to be heard.
    pub fn is_sounding(&self) -> bool {
        self.wait > 0 || self.pos < self.sound_ends_at()
    }

    /// How many stations are calling across it right now: the band's own
    /// traffic makes room for them while they are on the air.
    pub fn callers_now(&self) -> usize {
        if self.is_sounding() { self.callers } else { 0 }
    }

    /// The next sample: what the receiver hears of it before its AGC, what
    /// of it is played past the receiver, and whether you are sending.
    pub fn next(&mut self) -> SendSample {
        if self.wait > 0 {
            self.wait -= 1;
            return SendSample::default();
        }
        let pos = self.pos;
        if pos == 0 {
            self.first_played = Some(Instant::now());
        }
        self.pos += 1;
        if pos >= self.sound_ends_at() {
            return SendSample::default();
        }
        let release = match self.released_at {
            Some(at) if pos >= at => release_gain((pos - at) as f64 / self.release_len as f64),
            _ => 1.0,
        };
        // A stop is the end of your sending, and the receiver comes back;
        // handing over to a send that goes on with your message, it is not.
        let keyed = (self.hands_over || self.released_at.is_none_or(|at| pos < at))
            && self.muted_until.is_some_and(|until| pos < until);
        let sr = self.sample_rate;
        let on_air: f32 = self
            .stations
            .iter_mut()
            .map(|station| station.sample(pos, sr))
            .sum();
        let heard = self.filter.process(f64::from(on_air));
        let dry = self
            .dry
            .as_mut()
            .map_or(0.0, |station| f64::from(station.sample(pos, sr)));
        let tail_from = self.end - self.tail_fade.min(self.end);
        let tail = if pos >= tail_from {
            release_gain((pos - tail_from) as f64 / self.tail_fade as f64)
        } else {
            1.0
        };
        // A stop takes your message down with the rest; the fading is the
        // path's, and yours never crossed it.
        SendSample {
            heard: heard * self.fading_gain(pos) * release * tail,
            dry: dry * release * tail,
            keyed,
        }
    }

    fn fading_gain(&mut self, pos: usize) -> f64 {
        let Some(fading) = &mut self.fading else {
            return 1.0;
        };
        if fading.left == 0 {
            let ahead = (pos + fading.every) as f64 / self.sample_rate;
            let target = f64::from(fading.qsb.gain_at(self.started_at_sec + ahead));
            fading.slope = (target - fading.level) / fading.every as f64;
            fading.left = fading.every;
        }
        let level = fading.level;
        fading.level += fading.slope;
        fading.left -= 1;
        level
    }

    /// Say the send is over once its last sample has been heard: `latency`
    /// samples after it was played, which is how far behind the callback the
    /// speaker is. Called once per callback, after the buffer.
    ///
    /// Until then it also says whether it is playing late: what it has
    /// played behind the wall clock since its first sample, as a stream held
    /// up between callbacks leaves it. A buffer renders ahead of the wall
    /// clock, never behind it, while the stream keeps up.
    pub fn settle(&self, latency: usize) {
        if self.is_finished() {
            return;
        }
        if let Some(first) = self.first_played {
            let played_ms = self.pos as f64 * 1_000.0 / self.sample_rate;
            let behind_ms = first.elapsed().as_secs_f64() * 1_000.0 - played_ms;
            if behind_ms > LATE_MS as f64 {
                self.late.store(true, Ordering::Release);
            }
        }
        if self.wait == 0 && self.pos >= self.sound_ends_at().saturating_add(latency) {
            self.finished.store(true, Ordering::Release);
        }
    }

    /// Played out, and heard: the mixer can let it go.
    pub fn is_finished(&self) -> bool {
        self.finished.load(Ordering::Acquire)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cw_core::{
        Interferer, SplitMix64, TrainingSettings, Transmission, plan_transmission, resolve_station,
    };

    const SR: u32 = 8_000;

    fn settings() -> TrainingSettings {
        let mut settings = TrainingSettings::default();
        settings.playback.char_wpm_min = 20.0;
        settings.playback.char_wpm_max = 20.0;
        settings.playback.effective_wpm_min = 20.0;
        settings.playback.effective_wpm_max = 20.0;
        settings.band.link_volume = true;
        settings.band.volume_min = 1.0;
        settings
    }

    fn planned(text: &str, others: &[&str]) -> PlannedTransmission {
        planned_with(&settings(), text, others)
    }

    fn planned_with(
        settings: &TrainingSettings,
        text: &str,
        others: &[&str],
    ) -> PlannedTransmission {
        let mut rng = SplitMix64::new(1);
        let voice = resolve_station(settings, &mut rng);
        let others = others
            .iter()
            .enumerate()
            .map(|(k, text)| Interferer {
                text: (*text).into(),
                voice: cw_core::StationVoice {
                    tone_hz: voice.tone_hz + 150.0 * (k + 1) as f64,
                    volume: 0.5,
                    ..voice
                },
                delay_sec: 0.1 * (k + 1) as f64,
            })
            .collect();
        plan_transmission(
            &Transmission {
                text: text.into(),
                voice,
                others,
            },
            settings,
        )
    }

    fn receiver() -> FilterDesign {
        FilterDesign::from_settings(&settings())
    }

    fn play(voice: &mut SendVoice, samples: usize) -> Vec<f32> {
        (0..samples).map(|_| voice.next().heard as f32).collect()
    }

    /// The send the callback generates is the one the player used to render
    /// whole before it started, sample for sample: every station's tones
    /// summed, through the receiver.
    #[test]
    fn a_send_is_the_stations_summed_through_the_receiver() {
        let plan = planned("PARIS", &["DL1ABC", "K"]);
        let sr = f64::from(SR);
        let length = (plan.wanted.duration_sec * sr).ceil() as usize;
        // Rendered whole, the way it used to be.
        let mut expected = vec![0.0f32; length + (TAIL_SEC * sr) as usize];
        for station in std::iter::once(&plan.wanted).chain(&plan.others) {
            for tone in &station.events {
                let start = (tone.start_sec * sr).round() as usize;
                let len = ((tone.duration_sec * sr).round() as usize).max(1);
                let last = (tone.envelope.len() - 1) as f64;
                for i in 0..len {
                    let t = i as f64 / sr;
                    let pos = (t / tone.duration_sec).clamp(0.0, 1.0) * last;
                    let k = pos.floor() as usize;
                    let frac = (pos - k as f64) as f32;
                    let a = tone.envelope[k];
                    let b = tone.envelope.get(k + 1).copied().unwrap_or(a);
                    let sample = (std::f64::consts::TAU * tone.frequency_hz * t).sin() as f32
                        * (a * (1.0 - frac) + b * frac);
                    if let Some(slot) = expected.get_mut(start + i) {
                        *slot += sample;
                    }
                }
            }
        }
        let mut filter = ReceiverFilter::new(SR, &receiver());
        for sample in &mut expected {
            *sample = filter.process(f64::from(*sample)) as f32;
        }

        let mut voice = SendVoice::new(&plan, &receiver(), None, SR, 0.0);
        let played = play(&mut voice, expected.len());
        let before_the_tail_fade = expected.len() - (TAIL_FADE_SEC * sr) as usize;
        assert_eq!(
            played[..before_the_tail_fade],
            expected[..before_the_tail_fade]
        );
        let peak = played.iter().fold(0.0f32, |a, s| a.max(s.abs()));
        assert!(peak > 0.1, "peak was {peak}");
    }

    /// The ringing after the last element dies away smoothly however
    /// narrow the receiver: the send never stops on a step.
    #[test]
    fn a_send_ends_without_a_step() {
        let mut narrow = settings();
        narrow.band.filter_width_hz = cw_core::FILTER_WIDTH_MIN;
        narrow.band.filter_shape = cw_core::FilterShape::Sharp;
        narrow.band.side_tone_min = 600.0;
        narrow.band.side_tone_max = 600.0;
        let plan = planned_with(&narrow, "T", &[]);
        let receiver = FilterDesign::from_settings(&narrow);
        let mut voice = SendVoice::new(&plan, &receiver, None, SR, 0.0);
        let end = voice.end;
        let played = play(&mut voice, end + 16);
        let last = voice.end - 1;
        let peak = played.iter().fold(0.0f32, |a, s| a.max(s.abs()));
        assert!(
            played[last].abs() < 1e-4 * peak,
            "the last sample was {} against a peak of {peak}",
            played[last]
        );
        assert!(played[voice.end..].iter().all(|s| *s == 0.0));
    }

    /// Fading follows the shared fade, read off the band's clock from where
    /// the send starts.
    #[test]
    fn the_fading_follows_the_band_clock() {
        let mut fast = settings();
        fast.band.qsb_enabled = true;
        fast.band.qsb_depth = 0.75;
        fast.band.qsb_rate_hz = 1.5;
        let qsb = Qsb::of(&fast.band).expect("fading");
        let plan = planned("TTTTTTTTTT", &[]);
        let mut plain = SendVoice::new(&plan, &receiver(), None, SR, 0.0);
        let mut faded = SendVoice::new(&plan, &receiver(), Some(qsb), SR, 40.0);
        for i in 0..SR as usize {
            let (a, b) = (plain.next().heard, faded.next().heard);
            if a.abs() > 1e-3 {
                let expected = f64::from(qsb.gain_at(40.0 + i as f64 / f64::from(SR)));
                assert!(
                    (b / a - expected).abs() < 3e-3,
                    "at {i}: {} against {expected}",
                    b / a
                );
            }
        }
    }

    /// A dry send keeps your message out of the receiver: it comes out as
    /// its tones were planned, sample for sample, neither filtered nor
    /// faded, while the others come through the receiver as they would
    /// without it. A stop takes it down with them.
    #[test]
    fn a_dry_message_is_played_past_the_receiver() {
        let mut fading = settings();
        fading.band.qsb_enabled = true;
        fading.band.qsb_depth = 0.75;
        let qsb = Qsb::of(&fading.band).expect("fading");
        let sr = f64::from(SR);
        let mut dry = planned("PARIS", &["K"]);
        dry.dry_wanted = true;
        let mut callers = dry.clone();
        callers.wanted.events.clear();
        let mut voice = SendVoice::new(&dry, &receiver(), Some(qsb), SR, 0.0);
        let mut alone = SendVoice::new(&callers, &receiver(), Some(qsb), SR, 0.0);
        let mut yours = Station::new(&dry.wanted, sr);
        let mut peak = 0.0f64;
        for pos in 0..(dry.wanted.duration_sec * sr) as usize {
            let sample = voice.next();
            assert_eq!(sample.heard, alone.next().heard, "at {pos}");
            assert_eq!(sample.dry, f64::from(yours.sample(pos, sr)), "at {pos}");
            peak = peak.max(sample.dry.abs());
        }
        assert!(peak > 0.25, "your message peaked at {peak}");

        let mut voice = SendVoice::new(&dry, &receiver(), None, SR, 0.0);
        // Into the dah of the P.
        let into: Vec<f64> = (0..SR as usize / 5).map(|_| voice.next().dry).collect();
        assert!(
            into[into.len() - 16..].iter().any(|s| s.abs() > 0.1),
            "it should be sounding"
        );
        voice.release();
        let release = voice.release_len;
        let after: Vec<f64> = (0..release + 16).map(|_| voice.next().dry).collect();
        assert!(after[..release].iter().any(|s| *s != 0.0), "it cut off");
        assert!(after[release..].iter().all(|s| *s == 0.0));
    }

    /// A stopped send rides down over the release, never stepping, and is
    /// over once it has.
    #[test]
    fn a_stopped_send_rides_down_instead_of_cutting() {
        let plan = planned("TTTTT", &[]);
        let mut voice = SendVoice::new(&plan, &receiver(), None, SR, 0.0);
        let played = play(&mut voice, SR as usize / 10);
        assert!(
            played.iter().any(|s| s.abs() > 0.05),
            "it should be sounding"
        );
        // How far the tone itself moves between samples.
        let steady = played
            .windows(2)
            .map(|w| (w[1] - w[0]).abs())
            .fold(0.0f32, f32::max);
        voice.release();
        let release = voice.release_len;
        let tail = play(&mut voice, release + 16);
        assert!(tail[release..].iter().all(|s| *s == 0.0));
        assert!(!voice.is_sounding());
        let biggest = std::iter::once(played[played.len() - 1])
            .chain(tail.iter().copied())
            .collect::<Vec<_>>()
            .windows(2)
            .map(|w| (w[1] - w[0]).abs())
            .fold(0.0f32, f32::max);
        assert!(
            biggest <= steady,
            "the release moved {biggest} in a sample, the tone {steady}"
        );
    }

    /// The band makes room for the callers sounding at once across a send,
    /// not for every call in it: two short calls one after the other are
    /// one, two long ones over each other two.
    #[test]
    fn a_send_makes_room_for_the_callers_sounding_at_once() {
        let apart = planned("TTTTT", &["E", "E"]);
        assert_eq!(apart.others.len(), 2);
        let voice = SendVoice::new(&apart, &receiver(), None, SR, 0.0);
        assert_eq!(voice.callers_now(), 1);
        let over = planned("TTTTT", &["TTT", "TTT"]);
        let voice = SendVoice::new(&over, &receiver(), None, SR, 0.0);
        assert_eq!(voice.callers_now(), 2);
    }

    /// A send is over when it has been heard, not when it has been played:
    /// the speaker is a buffer or more behind the callback. Its one caller,
    /// heard across it, is off the air with it.
    #[test]
    fn a_send_is_finished_once_it_has_been_heard() {
        let plan = planned("TTT", &["E"]);
        let mut voice = SendVoice::new(&plan, &receiver(), None, SR, 0.0);
        let finished = voice.finished_flag();
        assert_eq!(voice.callers_now(), 1);
        let latency = 400;
        let end = voice.end;
        play(&mut voice, end);
        voice.settle(latency);
        assert!(!finished.load(Ordering::Acquire), "still in the speaker");
        assert_eq!(voice.callers_now(), 0, "its callers are off the air");
        play(&mut voice, latency);
        voice.settle(latency);
        assert!(finished.load(Ordering::Acquire));
        assert!(voice.is_finished());
    }

    /// A send whose stream keeps up plays ahead of the wall clock, a buffer
    /// at a time; one held up between callbacks falls behind it, and is
    /// found late.
    #[test]
    fn a_send_held_up_between_callbacks_is_found_late() {
        let plan = planned("TTT", &[]);
        let mut steady = SendVoice::new(&plan, &receiver(), None, SR, 0.0);
        let late = steady.late_flag();
        play(&mut steady, SR as usize / 10);
        steady.settle(0);
        assert!(!late.load(Ordering::Acquire), "rendered ahead");

        let mut held = SendVoice::new(&plan, &receiver(), None, SR, 0.0);
        let late = held.late_flag();
        play(&mut held, 64);
        held.settle(0);
        std::thread::sleep(std::time::Duration::from_millis(LATE_MS + 50));
        play(&mut held, 64);
        held.settle(0);
        assert!(
            late.load(Ordering::Acquire),
            "held up for longer than a send may be"
        );
    }

    /// A send with nothing in it is still a send: it has its tail, and ends.
    #[test]
    fn a_send_with_nothing_in_it_still_ends() {
        let plan = planned("", &[]);
        let mut voice = SendVoice::new(&plan, &receiver(), None, 0, 0.0);
        assert!(voice.is_sounding());
        let end = voice.end;
        let played = play(&mut voice, end + 1);
        assert!(played.iter().all(|s| *s == 0.0));
        voice.settle(0);
        assert!(voice.is_finished());
    }

    #[test]
    fn the_envelope_is_read_off_the_curve() {
        let tone = ToneEvent {
            start_sec: 0.0,
            duration_sec: 1.0,
            frequency_hz: 600.0,
            target_gain: 0.5,
            envelope: vec![0.0, 1.0].into(),
        };
        assert_eq!(envelope_gain(&tone, 0.0), 0.0);
        assert_eq!(envelope_gain(&tone, 0.5), 0.5);
        assert_eq!(envelope_gain(&tone, 1.0), 1.0);
        // Outside the symbol the curve is clamped, not extrapolated.
        assert_eq!(envelope_gain(&tone, -1.0), 0.0);
        assert_eq!(envelope_gain(&tone, 9.0), 1.0);
        // A symbol with no curve holds its level.
        let flat = ToneEvent {
            envelope: vec![1.0].into(),
            target_gain: 0.25,
            ..tone
        };
        assert_eq!(envelope_gain(&flat, 0.5), 0.25);
        let instant = ToneEvent {
            duration_sec: 0.0,
            ..flat
        };
        assert_eq!(envelope_gain(&instant, 0.0), 0.25);
    }
}
