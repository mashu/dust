//! The receiver rendered ahead of the clock, a stretch at a time, and made
//! again from any moment: how the browser plays the very mixer the native
//! player runs.
//!
//! A browser plays what it is handed ahead of time, and nothing can run in
//! its audio thread sample by sample short of an AudioWorklet. So the mixer
//! is run ahead of the browser's clock, a tenth of a second at a time, each
//! stretch handed over to be played at its own moment, and a copy of the
//! mixer kept from the start of each. Whatever changes what is heard — a
//! send, a stop, a new band, the RF gain — is made at a moment just past
//! what the browser has taken already: the stretch that moment falls in is
//! played again from its copy up to there, sample for sample as it was, the
//! change made, and everything after it made anew. The band, the stations,
//! the receiver's filter and AGC, the mute while you send, the fades and the
//! fading are then the native player's own, not an imitation of them in
//! nodes.

use std::collections::VecDeque;
use std::sync::Arc;

use super::mixer::{Meter, Mixer, Swept};
use super::{METER_FRAME_SEC, MeterFrame};

/// How long each stretch is: long enough that a browser busy for a moment
/// has several queued, short enough that one remade is little work.
pub const STRETCH_SEC: f64 = 0.1;

/// The mixer, run ahead of a clock: what it has rendered, still to play,
/// each stretch with the node `N` it was handed over as. Everything is
/// counted in samples from the clock's moment the stream started at, so
/// stretches meet to the sample however long it runs.
pub struct ReceiverStream<N> {
    /// The mixer as it stands at `next`.
    mixer: Mixer,
    meter: Arc<Meter>,
    /// Frames read off the meter so far, and those read in one stretch.
    seen: usize,
    read: Vec<MeterFrame>,
    sample_rate: f64,
    frame_len: u64,
    stretch: usize,
    out: Vec<f32>,
    /// The clock's moment at sample 0.
    origin: f64,
    /// Where the next stretch starts.
    next: u64,
    queued: VecDeque<Queued<N>>,
    /// The meter's frames, each with the sample it ends at, not yet taken.
    frames: VecDeque<(u64, MeterFrame)>,
    /// Nothing to play: the mixer is quiet and its last stretch was
    /// silence, so none is rendered until something changes.
    idle: bool,
}

/// A stretch handed over, from sample `starts` to `ends`, and the mixer as
/// it stood before it.
struct Queued<N> {
    starts: u64,
    ends: u64,
    before: Mixer,
    node: N,
}

/// A stretch already handed over that a change takes back.
#[derive(Debug, PartialEq)]
pub enum Taken<N> {
    /// To stop at a moment, the rest of it made anew.
    StopAt(N, f64),
    /// Never to play.
    Never(N),
}

impl<N: Clone> ReceiverStream<N> {
    /// `mixer` from `now` on the clock it is played by.
    pub fn new(mixer: Mixer, now: f64) -> Self {
        let sample_rate = f64::from(mixer.sample_rate());
        let stretch = ((STRETCH_SEC * sample_rate).round() as usize).max(128);
        Self {
            meter: mixer.meter(),
            mixer,
            seen: 0,
            read: Vec::new(),
            sample_rate,
            frame_len: ((METER_FRAME_SEC * sample_rate) as u64).max(1),
            stretch,
            out: vec![0.0; stretch],
            origin: now,
            next: 0,
            queued: VecDeque::new(),
            frames: VecDeque::new(),
            idle: false,
        }
    }

    /// The mixer as it stands at the end of what has been rendered.
    pub fn mixer(&self) -> &Mixer {
        &self.mixer
    }

    /// Render stretches until the next starts at `until` or later, each
    /// handed to `play` with the moment it starts: from `soonest` at the
    /// earliest, the soonest a stretch can still start on time, should the
    /// clock have run past what was rendered. A stretch `play` will not take
    /// is not rendered either.
    pub fn render_until<E>(
        &mut self,
        soonest: f64,
        until: f64,
        mut play: impl FnMut(f64, &[f32]) -> Result<N, E>,
    ) -> Result<(), E> {
        // Fallen behind — a long task, a hidden page — it picks up from
        // there rather than playing into the past all at once.
        self.next = self.next.max(self.sample_at(soonest));
        while !self.idle && self.time(self.next) < until {
            let before = self.mixer.clone();
            let len = self.stretch;
            self.render(len, true);
            let node = match play(self.time(self.next), &self.out[..len]) {
                Ok(node) => node,
                Err(err) => {
                    // Played back to where it was, the frames with it.
                    let keep = self.next;
                    self.frames.retain(|(ends, _)| *ends <= keep);
                    self.mixer = before;
                    return Err(err);
                }
            };
            let ends = self.next + len as u64;
            self.queued.push_back(Queued {
                starts: self.next,
                ends,
                before,
                node,
            });
            self.next = ends;
            self.idle = self.mixer.is_quiet() && self.out[..len].iter().all(|s| *s == 0.0);
        }
        Ok(())
    }

    /// Make `change` at `at`, or as soon after it as anything is still to be
    /// rendered: what was rendered from there on is taken back, the stretch
    /// it falls in played again up to `at` exactly as it was. When the
    /// change was made, and what was taken back.
    pub fn change_at(
        &mut self,
        at: f64,
        change: impl FnOnce(&mut Mixer, &mut Swept),
    ) -> (f64, Vec<Taken<N>>) {
        let at = self.sample_at(at);
        let mut taken = Vec::new();
        if let Some(k) = self.queued.iter().position(|queued| queued.ends > at) {
            let mut later = self.queued.split_off(k);
            let first = later.pop_front().expect("one at k");
            let cut = at.max(first.starts);
            // Its frames from the cut on are made again; those before it
            // stand, and playing up to it again makes them a second time.
            self.frames.retain(|(ends, _)| *ends <= cut);
            self.mixer = first.before.clone();
            self.next = first.starts;
            self.render((cut - first.starts) as usize, false);
            self.next = cut;
            if cut > first.starts {
                taken.push(Taken::StopAt(first.node.clone(), self.time(cut)));
                self.queued.push_back(Queued { ends: cut, ..first });
            } else {
                taken.push(Taken::Never(first.node));
            }
            taken.extend(later.into_iter().map(|queued| Taken::Never(queued.node)));
        } else {
            // Nothing rendered from `at` on: the mixer stands no later than
            // it, and picks up there.
            self.next = self.next.max(at);
        }
        self.idle = false;
        let mut swept = Swept::new();
        change(&mut self.mixer, &mut swept);
        drop(swept);
        (self.time(self.next), taken)
    }

    /// The stretches over by `now`, let go of; and the meter's frames heard
    /// longer ago than the meter keeps its own ([`Meter::KEPT`]), as its
    /// ring lets them go, so that with no scope to read them none pile up.
    pub fn played_by(&mut self, now: f64) -> Vec<N> {
        let now = self.sample_at(now);
        let mut over = Vec::new();
        while self.queued.front().is_some_and(|queued| queued.ends <= now) {
            over.extend(self.queued.pop_front().map(|queued| queued.node));
        }
        let kept = self.frame_len * Meter::KEPT as u64;
        while self
            .frames
            .front()
            .is_some_and(|(ends, _)| ends + kept <= now)
        {
            self.frames.pop_front();
        }
        over
    }

    /// The meter's frames heard by `now`, oldest first, onto `into`.
    pub fn take_frames(&mut self, now: f64, into: &mut Vec<MeterFrame>) {
        let now = self.sample_at(now);
        while self.frames.front().is_some_and(|(ends, _)| *ends <= now) {
            into.extend(self.frames.pop_front().map(|(_, frame)| frame));
        }
    }

    /// Whether nothing is being rendered, there being nothing to hear.
    pub fn is_idle(&self) -> bool {
        self.idle
    }

    /// The clock's moment at `sample`.
    fn time(&self, sample: u64) -> f64 {
        self.origin + sample as f64 / self.sample_rate
    }

    /// The sample nearest the clock's moment `at`, none before the first.
    fn sample_at(&self, at: f64) -> u64 {
        let samples = ((at - self.origin) * self.sample_rate).round();
        if samples.is_finite() && samples > 0.0 {
            samples as u64
        } else {
            0
        }
    }

    /// Run the mixer over `len` samples from `next` into `out`, and read
    /// the frames it metered: kept, each with the sample it ends at, or,
    /// played again, dropped.
    fn render(&mut self, len: usize, keep_frames: bool) {
        if self.out.len() < len {
            self.out.resize(len, 0.0);
        }
        let out = &mut self.out[..len];
        out.fill(0.0);
        self.mixer.render(out, 0);
        self.read.clear();
        self.meter.read_since(&mut self.seen, &mut self.read);
        if !keep_frames {
            return;
        }
        // The last frame ended as many samples before the end as the meter
        // has taken of the next.
        let ends = self.next + len as u64 - (self.mixer.meter_phase() as u64).min(len as u64);
        let count = self.read.len() as u64;
        for (k, frame) in self.read.iter().enumerate() {
            let at = ends - (count - 1 - k as u64) * self.frame_len;
            self.frames.push_back((at, *frame));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::mixer::{BandVoice, SendVoice};
    use crate::audio::shape::Gains;
    use cw_core::band::FilterDesign;
    use cw_core::{SplitMix64, TrainingSettings, Transmission, plan_transmission, resolve_station};

    const SR: u32 = 16_000;

    fn settings() -> TrainingSettings {
        let mut settings = TrainingSettings::default();
        settings.band.qrn_enabled = false;
        settings
    }

    fn send(settings: &TrainingSettings, text: &str, at_sec: f64) -> SendVoice {
        let voice = resolve_station(settings, &mut SplitMix64::new(5));
        let plan = plan_transmission(&Transmission::alone(text, voice), settings);
        let receiver = FilterDesign::from_settings(settings);
        SendVoice::new(&plan, &receiver, None, SR, at_sec)
    }

    fn mixer(settings: &TrainingSettings) -> Mixer {
        let mut mixer = Mixer::new(SR);
        let mut swept = Swept::new();
        let band = BandVoice::for_settings(settings, SR, 7);
        mixer.set_band_at(band, Gains::of(settings), &mut swept);
        mixer.start(send(settings, "TEST", 0.0), &mut swept);
        mixer
    }

    /// What a browser plays of the stretches handed over, as the stream
    /// takes them back: each from its moment, cut short where a change
    /// stopped it, or not at all.
    #[derive(Default)]
    struct Browser {
        /// Each stretch: where it starts, in samples, its samples, and how
        /// many of them play.
        stretches: Vec<(usize, Vec<f32>, usize)>,
    }

    impl Browser {
        fn play(&mut self, starts: f64, samples: &[f32]) -> Result<usize, ()> {
            let at = (starts * f64::from(SR)).round() as usize;
            self.stretches.push((at, samples.to_vec(), samples.len()));
            Ok(self.stretches.len() - 1)
        }

        fn take_back(&mut self, taken: Vec<Taken<usize>>) {
            for taken in taken {
                match taken {
                    Taken::StopAt(i, at) => {
                        let at = (at * f64::from(SR)).round() as usize;
                        let stretch = &mut self.stretches[i];
                        stretch.2 = at - stretch.0;
                    }
                    Taken::Never(i) => self.stretches[i].2 = 0,
                }
            }
        }

        /// Everything played, from the start, to `samples` long.
        fn heard(&self, samples: usize) -> Vec<f32> {
            let mut out = vec![0.0f32; samples];
            let mut covered = vec![false; samples];
            for (at, stretch, plays) in &self.stretches {
                for (i, sample) in stretch[..*plays].iter().enumerate() {
                    if let Some(slot) = out.get_mut(at + i) {
                        assert!(!covered[at + i], "two stretches play at {}", at + i);
                        covered[at + i] = true;
                        *slot = *sample;
                    }
                }
            }
            out
        }
    }

    /// The meter's frames are held for a reader only as long as the meter
    /// keeps its own: a minute played with none taken, the scope not shown,
    /// holds no more than that, and a reader then gets the newest of them.
    #[test]
    fn untaken_frames_are_let_go_as_the_meters_own_are() {
        let settings = settings();
        let mut stream = ReceiverStream::new(mixer(&settings), 0.0);
        let mut browser = Browser::default();
        for second in 1..=60 {
            let now = f64::from(second);
            let _ = stream.render_until(now - 1.0, now, |starts, samples| {
                browser.play(starts, samples)
            });
            let _ = stream.played_by(now - 0.5);
        }
        let _ = stream.played_by(60.0);
        let mut frames = Vec::new();
        stream.take_frames(60.0, &mut frames);
        assert_eq!(frames.len(), Meter::KEPT, "frames held for no one");
    }

    /// A change made mid-stretch, the queue a good way ahead of it, plays
    /// exactly as the mixer would have played with the change made at that
    /// sample in the first place: the stretch it falls in played again to
    /// there as it was, no sample twice and none missing, and the rest made
    /// anew. And the meter's frames come one a frame, in order, none twice.
    #[test]
    fn a_change_mid_stretch_plays_as_if_made_there() {
        let settings = settings();
        let at = 0.5373;
        let changed = |mixer: &mut Mixer, swept: &mut Swept| {
            mixer.start_at(send(&settings, "EEE", at), None, 0, swept);
        };
        let mut stream = ReceiverStream::new(mixer(&settings), 0.0);
        let mut browser = Browser::default();
        stream
            .render_until(0.0, 1.0, |starts, samples| browser.play(starts, samples))
            .expect("played");
        let (made, taken) = stream.change_at(at, changed);
        let cut = (at * f64::from(SR)).round() as usize;
        assert_eq!((made * f64::from(SR)).round() as usize, cut);
        assert!(
            matches!(taken.first(), Some(Taken::StopAt(..))),
            "{taken:?}"
        );
        browser.take_back(taken);
        stream
            .render_until(0.0, 2.0, |starts, samples| browser.play(starts, samples))
            .expect("played");
        let total = 2 * SR as usize;
        let heard = browser.heard(total);

        let mut straight = mixer(&settings);
        let mut expected = vec![0.0f32; total];
        straight.render(&mut expected[..cut], 0);
        let mut swept = Swept::new();
        changed(&mut straight, &mut swept);
        straight.render(&mut expected[cut..], 0);
        let first_off = heard.iter().zip(&expected).position(|(a, b)| a != b);
        assert_eq!(first_off, None, "the stream parts from the mixer there");
        assert!(
            heard[cut..].iter().any(|s| *s != 0.0),
            "nothing after the change"
        );

        let mut frames = Vec::new();
        stream.take_frames(2.0, &mut frames);
        let frame_len = (METER_FRAME_SEC * f64::from(SR)) as usize;
        assert_eq!(frames.len(), total / frame_len, "a frame every frame");
        let (mut seen, mut metered) = (0, Vec::new());
        straight.meter().read_since(&mut seen, &mut metered);
        assert_eq!(frames, metered, "the frames as the mixer metered them");
    }

    /// A change before anything is queued from its moment picks up there;
    /// one at the start of a stretch takes that stretch back whole.
    #[test]
    fn a_change_between_stretches_takes_them_back_whole() {
        let settings = settings();
        let mut stream = ReceiverStream::new(mixer(&settings), 0.0);
        let mut browser = Browser::default();
        stream
            .render_until(0.0, 0.5, |starts, samples| browser.play(starts, samples))
            .expect("played");
        let (made, taken) = stream.change_at(0.2, |mixer, _| mixer.stop());
        assert_eq!(made, 0.2);
        assert_eq!(
            taken,
            [Taken::Never(2), Taken::Never(3), Taken::Never(4)],
            "from the third stretch on"
        );
        let (made, taken) = stream.change_at(9.0, |mixer, _| mixer.stop());
        assert_eq!(
            (made, taken),
            (9.0, Vec::new()),
            "nothing there to take back"
        );
    }

    /// Once there is nothing left to hear, nothing more is rendered, until
    /// something changes; and what has played is let go of.
    #[test]
    fn a_quiet_receiver_renders_nothing_until_woken() {
        let mut quiet = settings();
        quiet.band.noise_enabled = false;
        quiet.band.agc_enabled = false;
        let mut stream = ReceiverStream::new(Mixer::new(SR), 0.0);
        assert!(stream.mixer().is_quiet());
        let mut browser = Browser::default();
        stream
            .render_until(0.0, 1.0, |starts, samples| browser.play(starts, samples))
            .expect("played");
        assert!(stream.is_idle());
        assert_eq!(browser.stretches.len(), 1, "one stretch of silence");
        let (made, _) = stream.change_at(3.0, |mixer, swept| {
            mixer.start(send(&quiet, "E", 3.0), swept);
        });
        assert_eq!(made, 3.0);
        stream
            .render_until(3.0, 3.5, |starts, samples| browser.play(starts, samples))
            .expect("played");
        assert_eq!(browser.stretches[1].0, 3 * SR as usize, "from the change");
        assert!(
            browser.stretches.len() > 2,
            "the dit and what rings on after it"
        );
        assert!(stream.is_idle(), "quiet again once it has rung out");
        assert_eq!(stream.played_by(3.15), [0, 1]);
    }

    /// A session over, its send stopped and its band taken away, the stream
    /// goes quiet once the band has faded and the send's release has run
    /// out, though no change comes after to sweep either out.
    #[test]
    fn a_band_taken_away_leaves_the_stream_to_go_quiet() {
        let settings = settings();
        let mut stream = ReceiverStream::new(mixer(&settings), 0.0);
        let mut browser = Browser::default();
        stream
            .render_until(0.0, 1.0, |starts, samples| browser.play(starts, samples))
            .expect("played");
        assert!(!stream.is_idle());
        stream.change_at(1.0, |mixer, swept| {
            mixer.stop();
            mixer.set_band(None, swept);
        });
        stream
            .render_until(1.0, 2.0, |starts, samples| browser.play(starts, samples))
            .expect("played");
        assert!(stream.is_idle(), "the band faded, the send released");
    }
}
